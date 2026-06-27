//! `KeyboardView` — the root widget that lays out keys, dispatches gestures
//! to the engine, manages the swipe overlay, and renders the swipe trail.
//!
//! Visual stack (back to front), all inside one `gtk4::Overlay`:
//!   1. Key grid       — the actual letter buttons.
//!   2. Swipe trail    — Cairo overlay drawing the gesture path.
//!
//! The trail's DrawingArea has `can_target=false` so pointer events fall
//! through to the keyboard underneath.

use crate::config::Config;
use crate::gestures::swipe::{KeyHitbox, SwipeTracker};
use crate::keyboard::{
    engine::{EngineEvent, EngineInput},
    ButtonBehavior, Key, KeyAction, KeyboardEngine, KeypadState,
};
use crate::layouts::{Layout, Row};
use crate::gestures::recognizer::GestureRecognizer;
use crate::ui::key_widget::KeyWidget;
use crate::ui::swipe_trail::SwipeTrail;
use gtk4::prelude::*;
use parking_lot::Mutex;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tracing::{debug, info};

/// Process-wide desired keyboard height in logical px. Set by the
/// settings-applied callback and at boot from `cfg.layout.compact`.
/// Read by `rebuild_view` to pick a layer-surface height that matches
/// the layout's natural rendered size.
///
/// We use an atomic rather than threading the cfg through every
/// rebuild_view call site (there are ~7 of them, all in deeply nested
/// closures). The trade-off is process-wide state, but in practice
/// there's only ever one keyboard window so it's effectively fine.
static DESIRED_KEYBOARD_HEIGHT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(320);

/// Width-scale multiplier in thousandths (1000 = 1.0x). Stored as
/// integer because the std atomic for f32 isn't available; we
/// convert on read. Range is clamped to [950, 1100] (i.e. -5% to
/// Maximum logical width the keyboard renders at. On a phone the
/// screen is narrower than this so it has no effect; on a laptop /
/// wide monitor (full-width layer-shell anchor) the keyboard is
/// capped to this width and centered, so keys stay phone-sized
/// rather than stretching off the right edge. Single source of
/// truth for both the unit-width computation and the keyboard_box
/// pin-and-center logic in rebuild_view — they MUST agree or rows
/// get laid out for one width while keys are sized for another.
pub const MAX_KEYBOARD_WIDTH: i32 = 720;

/// +10%) by the settings panel — bounds enforced at write time so
/// reads don't need to clamp.
pub static WIDTH_SCALE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(1000);

pub fn set_width_scale(scale: f32) {
    // NaN guard: clamp(NaN, _, _) returns NaN, and `NaN as i32` is 0.
    // A scale-of-0 stored here would render a zero-width keyboard
    // (every unit_w computation multiplies by this thousandth). The
    // slider widget can't produce NaN, but a hand-edited config or
    // future programmatic caller could. Default any non-finite input
    // to 1.0 (1x scale).
    let safe = if scale.is_finite() { scale } else { 1.0 };
    // Range widened from [0.75, 1.25] to [0.5, 1.5] — user requested
    // more variability. 0.5x is half the normal width (one-handed
    // thumb typing on a phablet), 1.5x is comfortable on a tablet
    // or external display. Below 0.5 touch targets get unusably
    // small; above 1.5 the keyboard overflows most phone screens.
    let v = (safe.clamp(0.5, 1.5) * 1000.0).round() as i32;
    WIDTH_SCALE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Height-scale multiplier in thousandths (1000 = 1.0x). Applied
/// in desired_height_for: the final keyboard height is multiplied
/// by this. Range [0.5, 1.5] — ±50%.
pub static HEIGHT_SCALE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(1000);

pub fn set_height_scale(scale: f32) {
    // Same NaN guard as width — see set_width_scale.
    let safe = if scale.is_finite() { scale } else { 1.0 };
    // Range widened from [0.75, 1.25] to [0.5, 1.5] — user requested
    // more variability. 0.5x makes the keyboard half-height (useful
    // when most of the screen needs to stay visible), 1.5x gives
    // larger touch targets for accessibility. Same rationale as
    // width — below 0.5 the keys are too short to tap reliably.
    let v = (safe.clamp(0.5, 1.5) * 1000.0).round() as i32;
    HEIGHT_SCALE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// Bottom padding in logical pixels. Added below the keyboard so
/// users with larger hands (who hold the phone lower) can reach
/// the keys without stretching. 0..240 px range — widened from
/// 0..120 because the user requested more variability and 240px
/// is still well under typical phone screen height (~800px),
/// leaving room for the keyboard itself.
pub static BOTTOM_PAD: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);

pub fn set_bottom_pad(px: i32) {
    BOTTOM_PAD.store(px.clamp(0, 240), std::sync::atomic::Ordering::Relaxed);
}

/// Process-wide compact-mode flag. Read by `desired_height_for` so
/// rebuild_view can compute its own height from the current layout
/// without needing cfg threaded through. Updated at boot and from
/// the settings on_change callback alongside WIDTH/HEIGHT/BOTTOM_PAD.
pub static COMPACT_MODE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_compact_mode(on: bool) {
    COMPACT_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Process-wide current thumb-arc polygon. Same justification as
/// `DESIRED_KEYBOARD_HEIGHT` — avoids threading cfg through every
/// rebuild_view path. Set at boot from cfg, updated in the settings
/// on_change callback. Read by `compute_row_geometry` to decide how
/// much to compress each row.
///
/// Uses a Mutex (rather than atomic) because ThumbArc is a polygon —
/// can't fit in an atomic. The Mutex is contended only on settings
/// changes and during the rebuild, so contention is negligible.
static CURRENT_THUMB_ARC: parking_lot::Mutex<Option<crate::config::ThumbArc>> =
    parking_lot::const_mutex(None);

/// Process-wide key-repeat configuration. Read by every key's
/// gesture handler when the user begins a press, to decide whether
/// to schedule repeating dispatches. Same pattern as the other
/// process-wide config snapshots.
#[derive(Debug, Clone, Copy)]
struct KeyRepeatConfig {
    enabled: bool,
    delay_ms: u32,
    interval_ms: u32,
}

impl KeyRepeatConfig {
    const fn disabled() -> Self {
        Self {
            enabled: false,
            delay_ms: 400,
            interval_ms: 50,
        }
    }

    /// Return the timing this config implies for the given key action.
    /// Arrow keys get an override that's significantly faster than the
    /// default — they're the primary tool for cursor navigation in the
    /// arrows tray (the "swipe to cursor move page"), and a 400ms delay
    /// + 50ms interval feels sluggish for "move 5 chars left" type
    /// usage. The override is 200ms first-repeat + 35ms cadence, which
    /// makes a held arrow advance the cursor at ~28 chars/sec —
    /// fast enough for navigation, slow enough that a deliberate tap
    /// doesn't accidentally repeat.
    ///
    /// Backspace and other repeatable actions keep the safer defaults.
    fn for_action(self, action: &crate::keyboard::KeyAction) -> Self {
        use crate::keyboard::KeyAction;
        let is_arrow = matches!(
            action,
            KeyAction::Keysym(name)
                if matches!(name.as_str(), "Up" | "Down" | "Left" | "Right")
        );
        if is_arrow {
            Self {
                enabled: self.enabled,
                delay_ms: 200,
                interval_ms: 35,
            }
        } else {
            self
        }
    }
}

static KEY_REPEAT_CONFIG: parking_lot::Mutex<KeyRepeatConfig> =
    parking_lot::const_mutex(KeyRepeatConfig::disabled());

/// Process-wide "swipe gesture has had real movement" flag. Set by
/// `attach_swipe_handler`'s drag_update callback the first time
/// cumulative finger movement crosses `TAP_EVICTION_PX`. Cleared at
/// drag_end (any path).
///
/// Read by every key widget's click handler — if set, the synchronous
/// click event is dropped. This is what prevents rotation gestures
/// (which involve real movement but happen to release inside the
/// source button's bounds) from firing both the rotation result AND
/// the source key's tap character.
///
/// Importantly, this flag is NOT set on drag_begin. GTK fires
/// drag_begin as soon as the finger moves past the platform's
/// dnd-drag-threshold (~5px), which is too low — finger jitter on
/// a tap can easily cross that without the user intending a swipe.
/// Setting the flag only after `TAP_EVICTION_PX` of cumulative
/// movement ensures sub-threshold jitter doesn't break tap commits.
///
/// Eviction policy summary:
///   - press → no eviction yet
///   - small jitter (<TAP_EVICTION_PX) → no eviction
///   - real movement (≥TAP_EVICTION_PX) → flag set, evicts any
///     subsequent click event from this gesture cycle
///   - release → click event fires synchronously; tap handler
///     checks flag and either commits (flag clear) or drops
///     (flag set)
///   - drag_end → flag cleared for the next gesture cycle
pub static SWIPE_GESTURE_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Public read accessor for `SWIPE_GESTURE_ACTIVE`. Used by the
/// IM deactivate handler to decide whether to signal a focus-loss
/// abort. Only set the abort flag if a swipe is actually in
/// progress; otherwise a normal deactivate (user finished typing,
/// closed app) doesn't need to abort anything.
pub fn swipe_gesture_active() -> bool {
    SWIPE_GESTURE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Generation counter, retained as belt-and-suspenders for any
/// future patch that reintroduces a queued / asynchronous tap
/// pipeline. The current synchronous tap path doesn't read this;
/// the flag above is sufficient. Bumped on drag_update (when the
/// flag transitions false→true) and on drag_end-significant.
static TAP_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Signal from the IM-Deactivate handler to in-progress drags:
/// "focus left the field while you were swiping — abort the
/// dispatch when drag_end fires."
///
/// Set by `note_focus_lost_during_swipe` in the focus reconciler
/// when active=false AND SWIPE_GESTURE_ACTIVE is true (the user is
/// mid-swipe). Cleared at drag_begin.
///
/// Why this and not the FOCUS_GENERATION counter: Plasma Mobile
/// (v1) fires spurious activate/deactivate/activate cycles during
/// normal usage — SurroundingText updates, auto-focus settling,
/// app launch transitions. A generation-based check fired on every
/// such cycle and aborted swipes the user actually wanted. The
/// deactivate-only signal catches the real "focus moved AWAY" case
/// (the user tapped a different field, a notification took focus,
/// an app-switcher activated) without false-firing on the noise.
static SWIPE_INTERRUPTED_BY_FOCUS_LOSS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Signal from the IM layer that focus was lost mid-swipe. Idempotent.
pub fn note_focus_lost_during_swipe() {
    SWIPE_INTERRUPTED_BY_FOCUS_LOSS
        .store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn take_swipe_interrupted_by_focus_loss() -> bool {
    SWIPE_INTERRUPTED_BY_FOCUS_LOSS
        .swap(false, std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn clear_swipe_interrupted_by_focus_loss() {
    SWIPE_INTERRUPTED_BY_FOCUS_LOSS
        .store(false, std::sync::atomic::Ordering::Relaxed);
}

/// Force-clear the active-swipe latch. Called on keyboard-show as a
/// safety net: normally the drag `cancel` handler clears this when a
/// gesture is interrupted by focus loss, but not every compositor
/// reliably fires the gesture-cancel when the layer surface is
/// hidden. If the latch were stuck true across a hide/show, EVERY
/// tap would be suppressed and the keyboard would appear dead. This
/// guarantees each fresh show starts from a clean slate.
pub fn reset_swipe_latch() {
    SWIPE_GESTURE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    TAP_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Compute the natural surface height for a keyboard layout, given
/// the compact-mode flag. Returns a value suitable for
/// `set_default_height`.
///
/// Sizes calibrated for phones at 300% scaling: at compact=true, a
/// 4-row keyboard fits in ~180 logical px (~540 actual screen px),
/// which is roughly 25% of an 800-tall phone display.
/// Voice-type mode flag. When on, the keyboard shrinks to ~50% of
/// its normal height so the microterm + above-keyboard surface has
/// room to show wrapped context (multi-line code, paragraphs, etc).
/// Toggled by long-pressing the 🎤 key with shift, or by a settings
/// option. Off by default.
pub static VOICE_TYPE_MODE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn set_voice_type_mode(on: bool) {
    VOICE_TYPE_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn voice_type_mode() -> bool {
    VOICE_TYPE_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

fn desired_height_for(rows: usize, compact: bool) -> i32 {
    desired_height_for_named(rows, compact, "")
}

/// Variant that takes a layout name so layouts with a "small by
/// design" footprint (currently just `arrows`) can use shorter
/// rows AND opt out of the 120px floor. Without the name hint,
/// the 2-row arrows tray comes out at the 120px floor even though
/// the user explicitly wants a tighter navigation bar.
fn desired_height_for_named(rows: usize, compact: bool, layout_name: &str) -> i32 {
    let is_arrows = layout_name == "arrows";
    // Arrows tray uses the compact row height (38px) regardless of
    // the global compact-mode setting — it's a navigation surface,
    // not a typing surface, and the user asked to shrink it. Saves
    // ~16px per row vs the standard 46px.
    let key_h = if compact || is_arrows { 38 } else { 46 };
    let spacing = 3;
    let chrome = if is_arrows { 6 } else { 12 };
    let raw = rows as i32 * key_h + (rows as i32 - 1).max(0) * spacing + chrome;
    // Voice-type mode halves the keyboard. User-controlled scale
    // applies after.
    let h = if voice_type_mode() {
        (raw as f32 * 0.5) as i32
    } else {
        raw
    };
    // User height-scale multiplier (settings slider). Stored as
    // i32 thousandths.
    let scale_ths = HEIGHT_SCALE.load(std::sync::atomic::Ordering::Relaxed);
    let h = (h as f32 * scale_ths as f32 / 1000.0) as i32;
    // Bottom padding for users who hold the phone low and can't
    // reach the bottom of the screen comfortably.
    let bottom_pad = BOTTOM_PAD.load(std::sync::atomic::Ordering::Relaxed);
    // Floor: 120px for typing keyboards (ensures touch targets stay
    // tappable), 60px for arrows tray (it's a navigation bar — small
    // by design). Without the lower arrows-specific floor, the
    // 91px raw arrows height got clamped up to 120 even when the
    // user wanted it tight.
    let floor = if is_arrows { 60 } else { 120 };
    (h + bottom_pad).max(floor)
}

/// Compact-flag-from-atomic variant. Used by rebuild_view (which
/// doesn't have cfg in scope) so the surface height tracks the
/// CURRENT layout's row count + the live HEIGHT_SCALE / BOTTOM_PAD
/// / COMPACT_MODE settings, instead of whatever DESIRED_KEYBOARD_HEIGHT
/// happened to be set to at boot. Without this, transitions between
/// layouts of different row counts (stenotype 2-row primary → symbols
/// 4-row dip) leave the window at the wrong height — the symbols
/// page renders clipped because the surface was sized for stenotype.
pub fn desired_height_for_current_layout(rows: usize) -> i32 {
    desired_height_for_current_layout_named(rows, "")
}

/// Variant that takes a layout name so arrows tray gets its compact
/// row heights + lowered floor.
pub fn desired_height_for_current_layout_named(rows: usize, layout_name: &str) -> i32 {
    let compact = COMPACT_MODE.load(std::sync::atomic::Ordering::Relaxed);
    desired_height_for_named(rows, compact, layout_name)
}

pub struct KeyboardView {
    /// The outer container — `widget()` returns this. Holds the
    /// microterm on top and the keyboard overlay below.
    root: gtk4::Box,
    /// The Overlay container — Box(grid) underneath, SwipeTrail on top.
    overlay: gtk4::Overlay,
    /// The key grid container (lives inside the overlay).
    keyboard_box: gtk4::Box,
    /// The swipe trail overlay drawer.
    trail: SwipeTrail,
    /// Microterm view — the glanceable-info strip above the keys.
    /// Hidden by default; revealed for voice partials, status,
    /// terminal/web content (those last two pending vte4/webkit).
    microterm: Rc<crate::ui::microterm::MicrotermView>,
    engine: KeyboardEngine,
    cfg: Config,
    /// Currently displayed layout (qwerty, symbols, ...).
    current_layout: Rc<RefCell<Layout>>,
    /// Stashed letter-layout from before a SymbolsToggle / Numbers
    /// switch. Used by the engine's ABC-back-to-letters path.
    last_letter_layout: Rc<RefCell<Layout>>,
    /// Stashed layout from before swapping to the arrows tray
    /// (focus-tray mode). show_full_keyboard() restores from here.
    /// Separate from `last_letter_layout` because the arrows tray
    /// can be entered from EITHER letters or symbols, and we want
    /// to return to wherever the user actually was.
    pre_tray_layout: Rc<RefCell<Option<Layout>>>,
    /// All KeyWidgets currently in the view.
    key_widgets: Rc<RefCell<Vec<KeyWidget>>>,
    /// Tracker for swipe gestures.
    swipe: Rc<Mutex<SwipeTracker>>,
    /// Cache of key hitboxes recomputed on each swipe completion.
    hitboxes: Rc<RefCell<Vec<KeyHitbox>>>,
    /// Dictionary-backed swipe recognizer.
    recognizer: Rc<RefCell<GestureRecognizer>>,
    /// Visible-mode state: Full keyboard vs Tray (minimized) vs
    /// Dismissed. Shared by reference so the drag-end handler can flip
    /// it from inside its closure.
    visible_mode: Rc<Cell<VisibleMode>>,
}

/// Three possible visible states of the keyboard surface. See
/// `docs/single-hand-mode.md` for the full state-machine description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibleMode {
    /// Full letter keyboard, all features available.
    Full,
    /// Minimized tray (corner panel). Entered by long upward fling.
    Tray,
    /// Surface hidden entirely.
    Dismissed,
}

impl KeyboardView {
    pub fn new(engine: KeyboardEngine, cfg: Config) -> Self {
        // Side margins are deliberately generous (20px each) to give
        // swipe gestures room to overshoot past edge keys. A swipe that
        // exits the rightmost column lands in this margin band, which
        // is still inside the layer surface — so the GestureDrag
        // controller (attached to the overlay) keeps receiving update
        // events instead of clipping. Without this padding, a user
        // swiping "off the right edge" loses the tail of their swipe
        // path and the recognizer makes worse decisions.
        let keyboard_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(3)
            .css_classes(["keyboard-view"])
            .margin_start(8)
            .margin_end(8)
            .margin_top(2)
            .margin_bottom(2)
            .hexpand(true)
            .vexpand(true)
            .build();
        // If glass effect is enabled at construction time (e.g.
        // restored from config), tack on the .glass-keyboard class.
        // set_glass_effect() handles live toggles at runtime.
        if crate::ui::window::glass_effect_enabled() {
            keyboard_box.add_css_class("glass-keyboard");
        }

        // Microterm strip lives ABOVE the keyboard, OUTSIDE the
        // keyboard_box. This matters because rebuild_keys clears
        // keyboard_box's children — embedding the microterm there
        // would mean it gets deleted on every layout swap. Putting
        // it in the outer root_box (alongside the overlay) means
        // it's persistent across rebuilds.
        let microterm = Rc::new(crate::ui::microterm::MicrotermView::new());

        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&keyboard_box));
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        // Outer root box: microterm on top, then the keyboard
        // overlay underneath. This is what `widget()` returns.
        let root_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(0)
            .hexpand(true)
            .vexpand(true)
            .build();
        root_box.append(microterm.widget());
        root_box.append(&overlay);

        let trail = SwipeTrail::new();
        // Trail color: prefer the user's explicit swipe-trail pick
        // (colors.trail). Fall back to the legacy accent_color if the
        // user hasn't picked trail explicitly. The legacy field is
        // also the default for fresh configs.
        let trail_color = if !cfg.appearance.colors.trail.is_empty() {
            cfg.appearance.colors.trail.clone()
        } else {
            cfg.appearance.accent_color.clone()
        };
        trail.set_color_hex(&trail_color);
        // The trail floats above the keyboard but doesn't intercept pointer
        // events (set in the SwipeTrail constructor).
        overlay.add_overlay(trail.widget());

        // Canvas overlay: a thin Box that's hidden normally. When
        // the user swipes up on the spacebar (canvas_open=true), it
        // becomes visible with a ✕ on the left edge and ✓ on the
        // right edge — tall, easy thumb targets so the user can
        // commit or cancel without long-pressing space. The middle
        // of the screen is left clear (no widget) so the trail
        // draws over it and the user's strokes are visible.
        //
        // A 200ms timer polls canvas_open() and toggles the
        // overlay's visibility. Same idiom as the visibility
        // tracking we use elsewhere; cheap on idle (just an atomic
        // load + a no-op).
        let canvas_overlay_root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(0)
            .hexpand(true)
            .vexpand(true)
            .css_classes(["canvas-overlay"])
            .visible(false)
            // Don't intercept pointer events on the wrapper itself
            // — only the buttons do. Otherwise the user can't draw
            // over the middle area of the canvas because the Box
            // intercepts the swipe.
            .can_target(false)
            .build();
        // Left edge: cancel ✕. Tall button anchored to left edge,
        // narrow width so most of the canvas stays clear.
        let cancel_btn = gtk4::Button::builder()
            .label("✕")
            .css_classes(["canvas-cancel", "circular"])
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Center)
            .width_request(48)
            .height_request(120)
            .margin_start(4)
            // can_target=true override (the parent set false).
            .can_target(true)
            .build();
        // Middle filler — expands to push the right button to the
        // right edge. Doesn't intercept pointer events.
        let middle_spacer = gtk4::Box::builder()
            .hexpand(true)
            .can_target(false)
            .build();
        // Right edge: confirm ✓.
        let confirm_btn = gtk4::Button::builder()
            .label("✓")
            .css_classes(["canvas-confirm", "circular", "suggested-action"])
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Center)
            .width_request(48)
            .height_request(120)
            .margin_end(4)
            .can_target(true)
            .build();
        canvas_overlay_root.append(&cancel_btn);
        canvas_overlay_root.append(&middle_spacer);
        canvas_overlay_root.append(&confirm_btn);
        overlay.add_overlay(&canvas_overlay_root);

        // Wire ✕ → close canvas (universal cancel).
        cancel_btn.connect_clicked(|_| {
            tracing::debug!("canvas: cancel button tapped");
            crate::keyboard::character_draw::close_canvas();
            crate::ui::window::post_microterm_status("Canvas closed");
        });
        // Wire ✓ → recognize + commit + close. Same flow as the
        // long-press finalize path; factored to a helper so both
        // entry points stay in sync.
        let engine_for_confirm = engine.clone();
        confirm_btn.connect_clicked(move |_| {
            tracing::debug!("canvas: confirm button tapped");
            finalize_canvas(&engine_for_confirm);
        });

        // 200ms poll to toggle overlay visibility based on the
        // canvas_open() flag. We could also use a Cell-based event
        // notification, but for a single boolean and an infrequent
        // transition the polling timer is simpler.
        {
            let overlay_for_poll = canvas_overlay_root.clone();
            let last_visible: Rc<Cell<bool>> = Rc::new(Cell::new(false));
            glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                let should_be_visible =
                    crate::keyboard::character_draw::canvas_open();
                if should_be_visible != last_visible.get() {
                    last_visible.set(should_be_visible);
                    overlay_for_poll.set_visible(should_be_visible);
                }
                glib::ControlFlow::Continue
            });
        }

        let smoothing = cfg.gestures.smoothing;

        // Load the bundled dictionary at startup. We embed it in the binary
        // so a fresh install has working swipe recognition before the user
        // touches a config file.
        let dict_text = include_str!("../../resources/dictionary_en.txt");
        let words: Vec<String> = dict_text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|s| s.to_string())
            .collect();
        let recognizer = GestureRecognizer::with_dictionary(words);

        let initial_layout = resolve_letter_layout(&cfg);

        // Seed the process-wide height atomic from initial cfg, so the
        // first rebuild gets the right surface height (instead of the
        // 320 default baked into placement.rs).
        let initial_height = desired_height_for(initial_layout.rows.len(), cfg.layout.compact);
        DESIRED_KEYBOARD_HEIGHT.store(initial_height, std::sync::atomic::Ordering::Relaxed);

        // Seed the thumb-arc snapshot. None means "no compaction" —
        // full-width layout. Settings UI updates this when single-hand
        // mode toggles.
        *CURRENT_THUMB_ARC.lock() = cfg.layout.thumb_arc.clone();

        // Seed width-scale, height-scale, bottom-pad, compact-mode snapshots.
        set_width_scale(cfg.layout.width_scale);
        set_height_scale(cfg.layout.height_scale);
        set_bottom_pad(cfg.layout.bottom_pad_px);
        set_compact_mode(cfg.layout.compact);

        // Seed key-repeat config. The press handler reads this on
        // every press to decide whether to start a repeat timer.
        *KEY_REPEAT_CONFIG.lock() = KeyRepeatConfig {
            enabled: cfg.behavior.key_repeat,
            delay_ms: cfg.behavior.repeat_delay_ms,
            interval_ms: cfg.behavior.repeat_interval_ms,
        };

        let view = Self {
            root: root_box,
            overlay,
            keyboard_box,
            trail,
            microterm,
            engine,
            cfg,
            current_layout: Rc::new(RefCell::new(initial_layout.clone())),
            last_letter_layout: Rc::new(RefCell::new(initial_layout)),
            pre_tray_layout: Rc::new(RefCell::new(None)),
            key_widgets: Rc::new(RefCell::new(Vec::new())),
            swipe: Rc::new(Mutex::new(SwipeTracker::new(smoothing))),
            hitboxes: Rc::new(RefCell::new(Vec::new())),
            recognizer: Rc::new(RefCell::new(recognizer)),
            visible_mode: Rc::new(Cell::new(VisibleMode::Full)),
        };

        view.rebuild_keys();
        view.attach_swipe_handler();
        view.attach_engine_listener();
        view
    }

    pub fn widget(&self) -> gtk4::Widget {
        self.root.clone().upcast()
    }

    /// A cloneable handle to the microterm strip. Other subsystems
    /// (the voice bridge, a future status manager) keep this around
    /// and call `set_content` / `hide` on it from their own event
    /// loops. The Rc lets the handles outlive the KeyboardView's
    /// own closures.
    pub fn microterm_handle(&self) -> Rc<crate::ui::microterm::MicrotermView> {
        self.microterm.clone()
    }

    /// Shortcut: set the microterm content and reveal the strip.
    /// Equivalent to `microterm_handle().set_content(...)`.
    pub fn set_microterm_content(
        &self,
        content: crate::ui::microterm::MicrotermContent,
    ) {
        self.microterm.set_content(content);
    }

    /// Hide the microterm without disposing pages — re-showing
    /// later is cheap.
    pub fn hide_microterm(&self) {
        self.microterm.hide();
    }

    /// The keyboard's current visible mode (Full / Tray / Dismissed).
    pub fn visible_mode(&self) -> VisibleMode {
        self.visible_mode.get()
    }

    /// Switch to tray (minimized) mode. Replaces the keyboard's
    /// contents with a corner-anchored panel: expand / spacebar /
    /// close. The full keyboard is teardown'd; switching back via
    /// `enter_full()` rebuilds it.
    pub fn enter_tray(&self) {
        if self.visible_mode.get() == VisibleMode::Tray {
            return;
        }
        info!("KeyboardView: Full → Tray");
        self.visible_mode.set(VisibleMode::Tray);

        // Clear the keyboard_box and replace with the tray widget.
        while let Some(child) = self.keyboard_box.first_child() {
            self.keyboard_box.remove(&child);
        }
        self.key_widgets.borrow_mut().clear();

        let side = match self.cfg.layout.letters.as_str() {
            // Until thumb_arc lands, infer dominant side from… well,
            // we don't have a config field for it yet. Default to
            // Right; a future setting will let the user choose.
            _ => crate::ui::DominantSide::Right,
        };

        let visible_mode = self.visible_mode.clone();
        let kb_box = self.keyboard_box.clone();
        let widgets = self.key_widgets.clone();
        let cur_layout = self.current_layout.clone();
        let engine_cb = self.engine.clone();
        let recognizer = self.recognizer.clone();
        let on_expand: Rc<dyn Fn()> = Rc::new(move || {
            info!("tray: expand → Full");
            visible_mode.set(VisibleMode::Full);
            // Tear down the tray, rebuild the full keyboard.
            while let Some(child) = kb_box.first_child() {
                kb_box.remove(&child);
            }
            widgets.borrow_mut().clear();
            rebuild_view(&kb_box, &widgets, &cur_layout, &engine_cb, &recognizer);
        });

        let visible_mode2 = self.visible_mode.clone();
        let kb_box2 = self.keyboard_box.clone();
        let on_close: Rc<dyn Fn()> = Rc::new(move || {
            info!("tray: close → Dismissed");
            visible_mode2.set(VisibleMode::Dismissed);
            // Tell the parent window to hide. We do this by clearing
            // the keyboard_box and letting the parent's input-method
            // callback flow drive visibility. A cleaner approach
            // would be to emit a signal up, but we don't have one
            // yet; for now we just clear and let the upper layer
            // notice via window.set_visible.
            while let Some(child) = kb_box2.first_child() {
                kb_box2.remove(&child);
            }
            // Walk up to the top-level window and hide it.
            if let Some(root) = kb_box2.root() {
                if let Some(win) = root.dynamic_cast_ref::<gtk4::Window>() {
                    win.set_visible(false);
                }
            }
        });

        let tray = crate::ui::MinimizedTray::new(
            side,
            self.engine.clone(),
            crate::ui::minimized_tray::TrayCallbacks {
                on_expand,
                on_close,
            },
        );
        self.keyboard_box.append(tray.widget());
    }

    /// Switch from Tray (or Dismissed) back to Full keyboard mode.
    pub fn enter_full(&self) {
        if self.visible_mode.get() == VisibleMode::Full {
            return;
        }
        info!("KeyboardView: {:?} → Full", self.visible_mode.get());
        self.visible_mode.set(VisibleMode::Full);
        while let Some(child) = self.keyboard_box.first_child() {
            self.keyboard_box.remove(&child);
        }
        self.key_widgets.borrow_mut().clear();
        self.rebuild_keys();
    }

    /// Replace the current layout and rebuild all key widgets.
    pub fn set_layout(&self, layout: Layout) {
        *self.current_layout.borrow_mut() = layout;
        self.rebuild_keys();
    }

    /// Switch to the compact arrow-keys tray. Stashes the current
    /// layout in `pre_tray_layout` so `show_full_keyboard()` can
    /// restore it. No-op if already in tray.
    pub fn show_arrows_tray(&self) {
        let cur_name = self.current_layout.borrow().name.clone();
        if cur_name == "arrows" {
            return;
        }
        *self.pre_tray_layout.borrow_mut() =
            Some(self.current_layout.borrow().clone());
        *self.current_layout.borrow_mut() = Layout::default_arrows();
        // Sync engine state to match the visible layout. Without
        // this, the engine could be stuck in Symbols (if user was on
        // the symbols page when focus shifted) while the view shows
        // arrows. The next mode-toggle tap would then toggle from
        // the stuck state and nothing would happen visually.
        self.engine.set_keypad_state(
            self.current_layout.borrow().corresponding_keypad_state(),
        );
        self.rebuild_keys();
    }

    /// Restore the layout from before we entered tray mode. No-op
    /// if we don't have a saved pre-tray layout.
    pub fn show_full_keyboard(&self) {
        if self.current_layout.borrow().name != "arrows" {
            return;
        }
        let prev = self.pre_tray_layout.borrow_mut().take();
        if let Some(prev) = prev {
            *self.current_layout.borrow_mut() = prev;
            // Sync engine state — see show_arrows_tray for the
            // rationale. The restored layout might be a transient
            // page (e.g., the user was on symbols when focus took
            // them to arrows), in which case the engine should be
            // in the corresponding state.
            self.engine.set_keypad_state(
                self.current_layout.borrow().corresponding_keypad_state(),
            );
            self.rebuild_keys();
        }
    }

    /// Rebuild all key widgets from the current layout. Delegates to the
    /// free function `rebuild_view` so the same code runs from both
    /// `&self` (the constructor / `set_layout`) and from the engine
    /// listener closure (which has no `self`).
    fn rebuild_keys(&self) {
        rebuild_view(
            &self.keyboard_box,
            &self.key_widgets,
            &self.current_layout,
            &self.engine,
            &self.recognizer,
        );
    }

    /// Attach a `GestureDrag` to the keyboard so the user can swipe across
    /// keys. While the drag is active we feed every motion update to the
    /// SwipeTrail so the trail follows the finger live.
    fn attach_swipe_handler(&self) {
        let drag = gtk4::GestureDrag::new();
        drag.set_button(gtk4::gdk::BUTTON_PRIMARY);

        let cfg_threshold = self.cfg.gestures.swipe_threshold;

        // Source key behavior captured at drag_begin. The drag's
        // semantics are entirely determined by what kind of button
        // the user pressed:
        //
        //   - Normal: standard tap-or-cancel. Drags committed only
        //     on release-inside; drag-off cancels.
        //   - Cardinal: NESW/diagonal + clock-rotation + hairpin.
        //     dispatch_cardinal_swipe classifies and emits.
        //   - AltKeySwipe: one-cell-traversal swipe picks an alt
        //     character AND signals swipe-typing to abstain.
        //   - SwipeSource: swipe-typing starts immediately. Trail
        //     is drawn. drag_end runs the recognizer.
        //   - None (start hit no key): drag is ignored at release.
        //
        // Stored as Option<ButtonBehavior> in a Cell so the
        // drag_begin / drag_update / drag_end callbacks can share
        // the decision without re-running the hit-test.
        let source_behavior: Rc<Cell<Option<ButtonBehavior>>> = Rc::new(Cell::new(None));
        // Character-drawing mode: set when draw_mode is on AND the
        // drag started on the spacebar. When true, drag_update
        // appends points to a per-stroke accumulator (NOT the
        // swipe-typing tracker) and drag_end pushes the completed
        // stroke into the global stroke buffer for later OCR.
        // Tap-and-hold on the spacebar finalizes.
        let drag_drawing: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let drag_stroke_pts: Rc<RefCell<Vec<(f64, f64)>>> =
            Rc::new(RefCell::new(Vec::new()));

        // Coordinate frame translation: the drag controller is on
        // `self.overlay`, so gesture coords are in overlay frame. But
        // hitboxes are built via `widget.compute_bounds(keyboard_box)`,
        // which gives bounds in keyboard_box-local frame. These frames
        // differ by keyboard_box's offset within overlay (which is
        // roughly margin_start, margin_top = 8, 2 — shrunk from
        // 20/4 for 300%-scaled phone displays).
        //
        // We capture the offset at drag_begin by calling
        // `keyboard_box.compute_bounds(overlay)`. Then drag_begin and
        // drag_update subtract this offset from gesture coords before
        // storing in the swipe tracker — so the tracker (and all
        // downstream hit-tests) sees keyboard_box-frame coords.
        //
        // The trail widget is on the overlay, so it should keep
        // receiving overlay-frame coords (the un-translated ones).
        // That's a separate push from the tracker push.
        let kb_offset: Rc<Cell<(f64, f64)>> = Rc::new(Cell::new((0.0, 0.0)));

        // Begin
        {
            let swipe = self.swipe.clone();
            let trail = self.trail.clone();
            let widgets_begin = self.key_widgets.clone();
            let kb_box_begin = self.keyboard_box.clone();
            let overlay_begin = self.overlay.clone();
            let hitboxes_begin = self.hitboxes.clone();
            let cur_layout_begin = self.current_layout.clone();
            let behavior_for_begin = source_behavior.clone();
            let offset_for_begin = kb_offset.clone();
            let drag_drawing_begin = drag_drawing.clone();
            let drag_stroke_pts_begin = drag_stroke_pts.clone();
            let trail_for_begin = self.trail.clone();
            drag.connect_drag_begin(move |_gesture, x, y| {
                // Clear any stale "focus lost during swipe" signal
                // from a prior gesture so we don't pre-abort this
                // one. The flag is set by the IM deactivate handler
                // only when SWIPE_GESTURE_ACTIVE is true; under
                // normal flow drag_end consumes it via swap, but
                // we defensively clear here too in case a gesture
                // was abandoned without drag_end firing (rare but
                // possible on grab revocation).
                clear_swipe_interrupted_by_focus_loss();
                // Capture keyboard_box's offset within overlay. Use
                // compute_bounds because it accounts for margins,
                // halign, and any future positioning quirks.
                //
                // Failure mode: compute_bounds returns None when a
                // widget hasn't been laid out yet. On Plasma Mobile
                // the layer-surface can mapped-but-not-fully-sized
                // briefly during transitions; touches delivered in
                // that window get offset=(0,0) here, but the
                // hitboxes computed downstream are in the proper
                // post-margin KB-frame. The frame mismatch breaks
                // both swipe-typing (path lands on wrong row's
                // letters → empty word) and cardinal dispatch
                // (start-point hit-test against cells misses).
                //
                // Recover at drag_end by recomputing the offset and
                // correcting any drift in the captured path.
                let computed = kb_box_begin.compute_bounds(&overlay_begin);
                let offset = computed
                    .map(|b| (b.x() as f64, b.y() as f64))
                    .unwrap_or((0.0, 0.0));
                offset_for_begin.set(offset);

                // Translate gesture coords (overlay frame) into
                // keyboard_box frame for the swipe tracker.
                let kx = x - offset.0;
                let ky = y - offset.1;
                // Info-level so on-device journal pulls show the
                // offset. The kb_box has margins (start=8, top=2),
                // so offset==(0,0) here is a SIGNAL that
                // compute_bounds returned None and we're in the
                // bad-frame state.
                let offset_suspicious =
                    computed.is_none() || (offset.0 == 0.0 && offset.1 == 0.0);
                if offset_suspicious {
                    tracing::warn!(
                        "Drag begin: SUSPICIOUS offset ({:.1}, {:.1}) — \
                         compute_bounds returned {} at overlay ({:.1}, {:.1}). \
                         Path may be in wrong frame; drag_end will attempt recovery.",
                        offset.0, offset.1,
                        if computed.is_none() { "None" } else { "Some" },
                        x, y
                    );
                } else {
                    info!(
                        "Drag begin: overlay ({:.1}, {:.1}) → kb ({:.1}, {:.1}) \
                         [offset ({:.1}, {:.1})]",
                        x, y, kx, ky, offset.0, offset.1
                    );
                }
                swipe.lock().start(kx, ky);

                // Hit-test the start point against current hitboxes to
                // find the source key. The source key's `behavior()`
                // method tells us what kind of touch handler this drag
                // should use. We then apply a layout-level override:
                // in non-cardinal layouts, character keys act as
                // SwipeSource so dragging from any letter starts
                // swipe-typing (the classic QWERTY/Dvorak behavior).
                refresh_hitboxes(&widgets_begin, &kb_box_begin, &hitboxes_begin);
                let dispatch_mode =
                    cur_layout_begin.borrow().style.dispatch_mode();
                let suppress_freeform =
                    dispatch_mode.suppresses_freeform_swipe();
                let behavior = {
                    let boxes = hitboxes_begin.borrow();
                    let hit = boxes.iter().find(|b| b.contains(kx, ky)).map(|b| {
                        let raw = b.key.behavior();
                        if suppress_freeform {
                            // Experimental modes (cardinal/steno) must
                            // NOT run the freeform swipe-typing
                            // recognizer. The space key reports
                            // SwipeSource unconditionally (correct for
                            // QWERTY springboard), so swiping from the
                            // cardinal/steno space bar would otherwise
                            // fire swipe-typing and look like the normal
                            // keyboard took over. Downgrade SwipeSource
                            // → Normal here; the key's own declared
                            // Cardinal behavior (steno buttons, cardinal
                            // cells) is preserved so their swipes still
                            // route through dispatch_cardinal_swipe.
                            if raw == ButtonBehavior::SwipeSource {
                                ButtonBehavior::Normal
                            } else {
                                raw
                            }
                        } else if raw == ButtonBehavior::Normal
                            && matches!(b.key.action, KeyAction::Character)
                            && crate::gestures::swipe::swipe_typing_enabled()
                        {
                            // Standard mode WITH swipe-typing enabled:
                            // Normal letter keys participate in swipe-
                            // typing so dragging through them composes
                            // a word. When swipe-typing is off, leave
                            // them as Normal so drag-off-letter is a
                            // clean tap-or-cancel (no recognizer pass,
                            // no surprise word).
                            ButtonBehavior::SwipeSource
                        } else {
                            raw
                        }
                    });
                    // Regression fix (vs the OLD pre-rewrite code at
                    // 151b01d^): if no key was hit at drag_begin but
                    // we're in Standard mode with swipe-typing on,
                    // FALL BACK TO SwipeSource so the path still runs
                    // through the recognizer at drag_end. The OLD
                    // code never required a hit-test on the source
                    // key for non-cardinal layouts (`treat = true`
                    // unconditionally in non-cardinal); the rewrite
                    // started requiring `Some(behavior)` and silently
                    // dropped any gesture whose start landed outside
                    // every hitbox. Real-world misses include:
                    //   - touch landing in the bottom padding strip
                    //     below the bottom row
                    //   - touch landing in a gap between rows
                    //   - compute_bounds returning bad/zero offset
                    //     on a slow compositor so the translated
                    //     kx/ky misses every box
                    // dispatch_swipe_source operates on path geometry,
                    // not on a source-key reference, so the fall-back
                    // is safe.
                    if hit.is_none()
                        && !suppress_freeform
                        && crate::gestures::swipe::swipe_typing_enabled()
                    {
                        Some(ButtonBehavior::SwipeSource)
                    } else if hit.is_none() && suppress_freeform {
                        // Cardinal / Stenotype off-cell start: fall
                        // back to Cardinal behavior so drag_end runs
                        // dispatch_cardinal_swipe. That function does
                        // its own nearest-cell hit-test (within 30px)
                        // and resolves the source key from a touch
                        // that landed in the gap between cells. This
                        // is the "cardinal swipes don't work" report:
                        // CSS margins between cardinal cells create a
                        // few-pixel band where touch coords miss every
                        // widget but the user clearly intended to
                        // swipe FROM the nearest cell.
                        Some(ButtonBehavior::Cardinal)
                    } else {
                        hit
                    }
                };
                debug!(
                    "drag begin: source behavior = {:?} (at kb-frame {:.1}, {:.1}, mode={:?})",
                    behavior, kx, ky, dispatch_mode
                );
                behavior_for_begin.set(behavior);

                // Debug trail styling: reflect the gesture category in
                // the live trail so it's visually obvious on-device
                // which branch this gesture is heading for. No-op
                // visually unless debug styling is enabled (the draw
                // path forces Solid otherwise).
                let style = match behavior {
                    Some(ButtonBehavior::Cardinal) => {
                        crate::ui::swipe_trail::TrailStyle::Dotted
                    }
                    Some(ButtonBehavior::SwipeSource) => {
                        crate::ui::swipe_trail::TrailStyle::Striped
                    }
                    Some(ButtonBehavior::Normal) | None => {
                        crate::ui::swipe_trail::TrailStyle::Desaturated
                    }
                    _ => crate::ui::swipe_trail::TrailStyle::Solid,
                };
                trail_for_begin.set_style(style);

                // Character-drawing mode: if the global flag is on
                // AND the user started this drag on the spacebar,
                // route the gesture into stroke accumulation rather
                // than swipe-typing. The spacebar's normal swipe
                // path-decoder is bypassed; the trail draws as
                // usual so the user sees what they're inking.
                let source_is_space = {
                    let boxes = hitboxes_begin.borrow();
                    boxes
                        .iter()
                        .find(|b| b.contains(kx, ky))
                        .map(|b| matches!(b.key.action, KeyAction::Space))
                        .unwrap_or(false)
                };
                // Two ways to enter stroke-accumulation:
                //   1. Persistent draw_mode toggle + drag starts on space
                //   2. One-shot canvas_open (after swipe-up on space) —
                //      ANY drag on the keyboard accumulates as stroke
                //
                // In mode 2 the user has explicitly invoked the canvas
                // and is going to confirm/cancel via overlay buttons,
                // so we route everything to strokes.
                let canvas_open = crate::keyboard::character_draw::canvas_open();
                let drawing_now = canvas_open
                    || (crate::keyboard::character_draw::draw_mode_enabled()
                        && source_is_space);
                drag_drawing_begin.set(drawing_now);
                if drawing_now {
                    drag_stroke_pts_begin.borrow_mut().clear();
                    drag_stroke_pts_begin.borrow_mut().push((kx, ky));
                    debug!(
                        "character-draw: stroke begin at ({:.1}, {:.1}) (canvas_open={})",
                        kx, ky, canvas_open
                    );
                }

                // NOTE: we do NOT set SWIPE_GESTURE_ACTIVE here.
                // GTK fires drag_begin as soon as the finger moves
                // past the platform's dnd-drag-threshold (usually
                // ~5px), which is well below the "this is a real
                // swipe" threshold. Setting the flag here would
                // suppress taps with sub-threshold finger jitter.
                //
                // Instead, the flag is set in drag_update when the
                // cumulative movement crosses TAP_EVICTION_PX (see
                // below). That way a finger that wiggles 5-10px and
                // releases inside the key still commits the tap;
                // only a finger that actually traveled far enough
                // for a real swipe blocks the tap.

                // Trail visibility: only SwipeSource (spacebar in
                // springboard, or any letter in QWERTY/Dvorak) draws
                // the swipe-typing trail. Cardinal, AltKeySwipe, and
                // Normal-non-letter all suppress the trail because
                // they're per-key gestures whose path-drawing would
                // visually suggest the wrong thing (looks like
                // swipe-type, isn't).
                trail.clear();
                if matches!(behavior, Some(ButtonBehavior::SwipeSource)) {
                    trail.push(x, y);
                }
            });
        }

        // Update — append each new sample to the tracker AND the trail.
        // ALSO: the first time cumulative movement crosses the
        // "this is a real swipe, not finger jitter" threshold, set
        // SWIPE_GESTURE_ACTIVE so the per-key tap handler knows to
        // drop any subsequent click event from this gesture cycle.
        //
        // We use a separate threshold (TAP_EVICTION_PX) from
        // is_significant_swipe's (cfg.gestures.swipe_threshold,
        // default 20px) because we want to evict the tap EARLIER
        // than we'd classify the swipe as significant — so a long
        // wandering gesture that ends inside the source key never
        // commits the tap, even if its final straight-line distance
        // happens to be small.
        {
            let swipe = self.swipe.clone();
            let trail = self.trail.clone();
            let behavior_for_update = source_behavior.clone();
            let offset_for_update = kb_offset.clone();
            let drag_drawing_update = drag_drawing.clone();
            let drag_stroke_pts_update = drag_stroke_pts.clone();
            drag.connect_drag_update(move |gesture, dx, dy| {
                if let Some((sx, sy)) = gesture.start_point() {
                    let overlay_x = sx + dx;
                    let overlay_y = sy + dy;
                    let (ox, oy) = offset_for_update.get();
                    let kx = overlay_x - ox;
                    let ky = overlay_y - oy;

                    // Character-drawing branch: accumulate the
                    // point into the current stroke and draw the
                    // trail. Skip swipe-type tracking AND tap
                    // eviction — this gesture is neither a tap
                    // nor a swipe-word.
                    if drag_drawing_update.get() {
                        // Sample-distance filter (~3px) to keep
                        // the stroke from blowing up on a 200Hz
                        // touchscreen. Same threshold as the
                        // trail push below. Squared-distance
                        // compare avoids the sqrt — 3px → 9px²
                        // comparison; mathematically equivalent
                        // since both sides are positive.
                        let mut pts = drag_stroke_pts_update.borrow_mut();
                        let last = pts.last().copied().unwrap_or((kx, ky));
                        let dx_l = kx - last.0;
                        let dy_l = ky - last.1;
                        let d_sq = dx_l * dx_l + dy_l * dy_l;
                        if d_sq >= 9.0 {
                            pts.push((kx, ky));
                            trail.push(overlay_x, overlay_y);
                        }
                        return;
                    }

                    // Tap-eviction check: have we moved enough from
                    // the start point that this is clearly a swipe
                    // and not a jittery tap? Use the straight-line
                    // distance from start (== sqrt(dx² + dy²)). The
                    // tracker also accumulates path length but
                    // that's per-sample cumulative; for eviction we
                    // want the simpler "did the finger leave the
                    // starting region?" check.
                    if !SWIPE_GESTURE_ACTIVE
                        .load(std::sync::atomic::Ordering::Relaxed)
                    {
                        // Squared-distance comparison: skip sqrt on
                        // every motion event before tap-eviction
                        // fires. Once evicted, this whole branch
                        // doesn't run, so the sqrt was only needed
                        // for the threshold compare — and we can
                        // compare squares just as easily. Per drag
                        // at 120Hz, this saves ~12-30 sqrts before
                        // eviction. Negligible individually but the
                        // drag_update closure is one of the hottest
                        // paths in the keyboard.
                        const TAP_EVICTION_PX: f64 = 12.0;
                        const TAP_EVICTION_SQ: f64 =
                            TAP_EVICTION_PX * TAP_EVICTION_PX;
                        let dist_sq = dx * dx + dy * dy;
                        if dist_sq >= TAP_EVICTION_SQ {
                            // Compute actual distance only for the
                            // log line. tracing's macros short-
                            // circuit if the level is filtered, so
                            // typical users don't pay the sqrt;
                            // debug-build users see the readable
                            // pixel value.
                            debug!(
                                "tap eviction: drag moved {:.1}px (>= {}px); \
                                 setting SWIPE_GESTURE_ACTIVE",
                                dist_sq.sqrt(), TAP_EVICTION_PX
                            );
                            SWIPE_GESTURE_ACTIVE
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                            TAP_GENERATION
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }

                    // swipe.update filters samples < 4px from the
                    // last one. Do the same for trail.push to keep
                    // redraw cost in line with the actual gesture
                    // rate — without this, on a 200Hz touchscreen we'd
                    // queue 200 redraws/sec on phone and the trail
                    // would lag. The bool return from update() skips
                    // the redundant current_path().len() compare that
                    // was previously needed to detect a push.
                    let pushed_to_path = swipe.lock().update(kx, ky);
                    // Normally only SwipeSource draws a trail. With
                    // debug styling on, draw for ANY behavior so the
                    // category-specific style (dotted cardinal, etc.)
                    // is visible live while drawing.
                    let draw_trail = matches!(
                        behavior_for_update.get(),
                        Some(ButtonBehavior::SwipeSource)
                    ) || crate::ui::swipe_trail::trail_debug_styling();
                    if pushed_to_path && draw_trail {
                        trail.push(overlay_x, overlay_y);
                    }
                }
            });
        }

        // End — finalize, run decoding, send to engine, fade trail.
        {
            let swipe = self.swipe.clone();
            let engine = self.engine.clone();
            let hitboxes = self.hitboxes.clone();
            let widgets = self.key_widgets.clone();
            let kb_box = self.keyboard_box.clone();
            let overlay_end = self.overlay.clone();
            let trail = self.trail.clone();
            let recognizer = self.recognizer.clone();
            let current_layout = self.current_layout.clone();
            // For the tray transition: capture all the bits enter_tray
            // needs, then in the minimize branch build a tray inline
            // (same logic as the public enter_tray method but inline
            // because the closure can't hold a &KeyboardView).
            let visible_mode = self.visible_mode.clone();
            let kb_box_for_tray = self.keyboard_box.clone();
            let widgets_for_tray = self.key_widgets.clone();
            let cur_layout_for_tray = self.current_layout.clone();
            let engine_for_tray = self.engine.clone();
            let recognizer_for_tray = self.recognizer.clone();

            let behavior_for_end = source_behavior.clone();
            let offset_for_end = kb_offset.clone();
            let drag_drawing_end = drag_drawing.clone();
            let drag_stroke_pts_end = drag_stroke_pts.clone();
            drag.connect_drag_end(move |_gesture, dx, dy| {
                let total = (dx * dx + dy * dy).sqrt();

                // Character-drawing branch: finalize this stroke
                // into the global buffer (does NOT clear the
                // trail — we want the user to see the
                // accumulated drawing). Reset the per-drag flag
                // and stroke buffer. Bypass swipe-word decoding.
                if drag_drawing_end.get() {
                    let pts = drag_stroke_pts_end.borrow_mut().drain(..).collect::<Vec<_>>();
                    if !pts.is_empty() {
                        debug!(
                            "character-draw: stroke complete with {} points",
                            pts.len()
                        );
                        crate::keyboard::character_draw::push_stroke(pts);
                        // Update microterm with stroke count so the
                        // user knows the keyboard saw their input.
                        let count = crate::keyboard::character_draw::current_strokes()
                            .strokes
                            .len();
                        crate::ui::window::post_microterm_status(format!(
                            "✏ {} stroke{} — hold space to finalize",
                            count,
                            if count == 1 { "" } else { "s" }
                        ));
                    }
                    drag_drawing_end.set(false);
                    swipe.lock().cancel();
                    SWIPE_GESTURE_ACTIVE
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                // Focus-loss-during-swipe abort. The deactivate
                // handler sets a flag when focus leaves the field
                // while a swipe is in progress (the user tapped a
                // different field, a notification took focus, etc.).
                // Without aborting, drag_end would commit the word
                // to whatever field is focused NOW — potentially
                // routing a chat message into a search bar.
                //
                // We use a dedicated flag (set on Deactivate only)
                // rather than a generation counter (bumped on every
                // focus event) because Plasma Mobile fires spurious
                // activate/deactivate/activate cycles during normal
                // usage. A generation-based check aborted swipes
                // the user actually wanted — the "cardinal swipes
                // not firing" regression. The flag-based approach
                // catches the real focus-moved-away case without
                // false-positives.
                //
                // `take_swipe_interrupted_by_focus_loss` is a
                // swap(false), so reading it consumes the signal
                // for the next gesture.
                if take_swipe_interrupted_by_focus_loss() {
                    tracing::warn!(
                        "drag end: focus lost during swipe — aborting dispatch. \
                         The compositor sent Deactivate while the gesture was \
                         in progress, and no Activate came back to clear the \
                         flag before drag_end. If this fires for swipes you \
                         expected to commit, the Activate handler in window.rs \
                         may need tuning."
                    );
                    swipe.lock().cancel();
                    trail.clear();
                    SWIPE_GESTURE_ACTIVE
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    TAP_GENERATION
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                let mut tracker = swipe.lock();

                if !tracker.is_significant_swipe(cfg_threshold) {
                    debug!("Drag ended; not a significant swipe (total={:.1})", total);
                    // Cardinal dead-zone recovery: if the source key was
                    // Cardinal AND the user moved enough to evict the
                    // tap (drag_update set SWIPE_GESTURE_ACTIVE on
                    // crossing TAP_EVICTION_PX=12), but the total path
                    // length didn't reach `cfg_threshold` (default 20),
                    // we'd otherwise drop the gesture entirely — the
                    // click was suppressed, and the swipe is below the
                    // threshold for dispatch. That's the "steno swipes
                    // not working" symptom: short directional flicks
                    // on a steno cell exit click territory but don't
                    // reach swipe territory.
                    //
                    // For Cardinal-behavior keys we have a classifier
                    // that already handles tiny directional swipes
                    // correctly (its FLICK branch covers exactly this
                    // 12-20px zone). Let it decide. If the gesture is
                    // actually tap-like, classify returns
                    // CardinalGesture::Tap and dispatch_cardinal_swipe
                    // early-returns without committing — equivalent to
                    // the current drop behavior.
                    let behavior = behavior_for_end.get();
                    let was_evicted = SWIPE_GESTURE_ACTIVE
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if was_evicted && matches!(behavior, Some(ButtonBehavior::Cardinal)) {
                        let raw_path = tracker.finish();
                        drop(tracker);
                        SWIPE_GESTURE_ACTIVE
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        TAP_GENERATION
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        refresh_hitboxes(&widgets, &kb_box, &hitboxes);
                        let boxes = hitboxes.borrow().clone();
                        // Coordinate-frame recovery: recompute offset
                        // at drag_end. If drag_begin's compute_bounds
                        // returned None, offset was (0, 0) and the
                        // captured path is in overlay frame, not
                        // KB frame. The hitboxes are always in KB
                        // frame, so a frame mismatch silently breaks
                        // the cardinal hit-test. Apply the delta
                        // here to bring path to KB frame.
                        let path = recover_frame(
                            raw_path,
                            offset_for_end.get(),
                            &kb_box,
                            &overlay_end,
                        );
                        debug!(
                            "cardinal dead-zone recovery: path={} points, behavior=Cardinal",
                            path.len()
                        );
                        dispatch_cardinal_swipe(&path, &boxes, &engine);
                        trail.fade_out();
                        return;
                    }
                    tracker.cancel();
                    trail.clear();
                    // Clear the active flag — this gesture is over.
                    // The flag may have been set during drag_update
                    // if cumulative movement crossed TAP_EVICTION_PX
                    // (in which case the click from this same
                    // press-release has already been suppressed), or
                    // it may have stayed false the whole time (in
                    // which case the click already committed the tap
                    // synchronously). Either way, we clear here so
                    // the NEXT gesture starts from a clean slate.
                    SWIPE_GESTURE_ACTIVE
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                let raw_path = tracker.finish();
                drop(tracker);

                // Clear active flag — this gesture is over. The flag
                // was set by drag_update during this drag (since we
                // got past is_significant_swipe, movement clearly
                // exceeded TAP_EVICTION_PX). Future events see
                // un-active.
                SWIPE_GESTURE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
                // Belt-and-suspenders generation bump in case any
                // future patch reintroduces a tap queue.
                TAP_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // Hitboxes might have been refreshed at drag_begin
                // (in cardinal mode) — but doing it again is cheap
                // and ensures correctness if a rebuild fired mid-drag
                // (rare but possible if engine events arrive during
                // a long swipe). Safer to refresh again.
                refresh_hitboxes(&widgets, &kb_box, &hitboxes);
                let boxes = hitboxes.borrow().clone();

                // Coordinate-frame recovery: see the comment in the
                // dead-zone branch above. Brings the path into the
                // KB frame (where the hitboxes live) if drag_begin
                // captured a bogus offset.
                let path = recover_frame(
                    raw_path,
                    offset_for_end.get(),
                    &kb_box,
                    &overlay_end,
                );

                // Log the swipe summary once. Helps on-phone diagnosis:
                // a single line tells you (start, end, dx, dy, path_len)
                // for every drag that exceeded the threshold. Useful for
                // identifying whether the path is what we expect AND
                // for verifying gesture dispatch picks the right branch.
                let path_start = path.first().copied();
                let path_end = path.last().copied();
                let behavior = behavior_for_end.get();
                if let (Some(s), Some(e)) = (path_start, path_end) {
                    debug!(
                        "drag end: behavior={:?} ({:.0},{:.0}) → ({:.0},{:.0}) \
                         dx={:.1} dy={:.1} total={:.1} points={}",
                        behavior,
                        s.x,
                        s.y,
                        e.x,
                        e.y,
                        dx,
                        dy,
                        total,
                        path.len(),
                    );
                }

                // Dispatch on the source button's behavior. Each behavior
                // is self-contained: it handles the gesture completely
                // or aborts. No fall-through. The old "linear gesture
                // taxonomy" was prone to false-positives (e.g. a
                // top-row cardinal north-swipe would match the
                // digit-detection branch and produce "2" instead of
                // running through dispatch_cardinal_swipe).
                // BEFORE per-behavior dispatch: check for a "dismiss
                // the keyboard" flick. This runs for ALL behaviors so
                // cardinal/stenotype users can also flick-to-dismiss
                // — previously the minimize check only ran inside
                // dispatch_swipe_source, leaving experimental-mode
                // users with no swipe-based dismiss path.
                //
                // strict_mode: cardinal/stenotype have their own
                // swipe semantics (cardinal-direction commits, steno
                // pair-strokes). A casual upward cell-swipe shouldn't
                // dismiss the keyboard. Strict mode requires a much
                // faster + longer flick that's clearly distinct from
                // any cardinal swipe (which stays roughly within one
                // cell ~80-120px).
                let dispatch_mode_for_dismiss =
                    current_layout.borrow().style.dispatch_mode();
                let strict_dismiss =
                    dispatch_mode_for_dismiss.suppresses_freeform_swipe();
                if detect_minimize_fling(&path, &boxes, dx, dy, strict_dismiss)
                    .is_some()
                {
                    info!("minimize fling detected (strict={}) — entering tray", strict_dismiss);
                    enter_tray_inline(
                        &visible_mode,
                        &kb_box_for_tray,
                        &widgets_for_tray,
                        &cur_layout_for_tray,
                        &engine_for_tray,
                        &recognizer_for_tray,
                    );
                    trail.fade_out();
                    return;
                }

                // Bottom-pad cursor-nav swipe → arrow key. See
                // `detect_bottom_pad_arrow` for the logic. Refactored
                // into a free function so regression tests can drive
                // it directly with synthetic path/box inputs.
                if let Some(start) = path.first() {
                    if let Some(end) = path.last() {
                        if let Some(arrow) =
                            detect_bottom_pad_arrow(start, end, &boxes, dx, dy)
                        {
                            info!(
                                "bottom-pad arrow swipe: {} \
                                 (start.y={:.0}, end.y={:.0})",
                                arrow, start.y, end.y
                            );
                            engine.handle_input(
                                EngineInput::NamedKey(arrow.into())
                            );
                            trail.fade_out();
                            return;
                        }
                    }
                }

                match behavior {
                    None => {
                        // Drag started outside any key. Nothing sensible
                        // to do at release — drop the gesture quietly.
                        debug!("drag end: no source key; ignoring");
                    }
                    Some(ButtonBehavior::Normal) => {
                        // Normal-tap behavior on a drag: if the user
                        // released inside the source key bounds, fire
                        // the tap. Otherwise the drag is a cancel.
                        //
                        // This handles the common "I started pressing
                        // a key, then dragged off because I changed my
                        // mind" pattern without registering the
                        // accidental keystroke.
                        dispatch_normal_drag(&path, &boxes, &engine);
                    }
                    Some(ButtonBehavior::Cardinal) => {
                        // Cardinal-grid cell: classify and dispatch.
                        // dispatch_cardinal_swipe handles NESW + diagonal
                        // swipes, hairpin (return-to-start), and clock
                        // rotations. Aborts on unrecognized paths
                        // without committing anything.
                        info!("drag end branch: cardinal dispatch");
                        dispatch_cardinal_swipe(&path, &boxes, &engine);
                    }
                    Some(ButtonBehavior::AltKeySwipe) => {
                        // One-cell-traversal swipe → alt-character.
                        // Not yet emitting to engine — this is a
                        // scaffold for future iOS-style flick-to-alt
                        // gestures. Logs diagnostic info for now.
                        dispatch_alt_key_swipe(&path, &boxes, &engine);
                    }
                    Some(ButtonBehavior::SwipeSource) => {
                        // Springboard / swipe-typing path. Existing
                        // recognizer pipeline runs here. Also where
                        // the spacebar-arrow swipe and minimize-fling
                        // are detected — both are SwipeSource-only
                        // gestures that fall out of the recognizer's
                        // segmentation logic when the path stays
                        // entirely within the keyboard.
                        dispatch_swipe_source(
                            &path,
                            &boxes,
                            &engine,
                            &recognizer,
                            &widgets,
                            &current_layout,
                            &visible_mode,
                            &kb_box_for_tray,
                            &widgets_for_tray,
                            &cur_layout_for_tray,
                            &engine_for_tray,
                            &recognizer_for_tray,
                            dx,
                            dy,
                        );
                    }
                }

                trail.fade_out();
            });
        }

        // Cancel handler. GTK fires `cancel` (NOT `drag-end`) when the
        // gesture's pointer sequence is taken away mid-drag — focus
        // loss, a SECOND finger claiming the sequence (GestureSingle
        // only tracks one), or the compositor revoking the input grab
        // (e.g. a notification steals focus). Without this, drag_begin's
        // setup is stranded: SWIPE_GESTURE_ACTIVE stays true and
        // SUPPRESSES every subsequent tap until another full drag
        // completes — a nasty "keyboard went dead" state during
        // testing. We mirror drag_end's cleanup minus the
        // commit/decode (a cancelled gesture produces no output).
        {
            let swipe_cancel = self.swipe.clone();
            let trail_cancel = self.trail.clone();
            let drag_drawing_cancel = drag_drawing.clone();
            let drag_stroke_pts_cancel = drag_stroke_pts.clone();
            drag.connect_cancel(move |_gesture, _sequence| {
                debug!("swipe drag cancelled — cleaning up stranded gesture state");
                swipe_cancel.lock().cancel();
                trail_cancel.clear();
                // If we were mid character-draw stroke, drop the
                // partial stroke (don't commit a half-drawn glyph).
                drag_drawing_cancel.set(false);
                drag_stroke_pts_cancel.borrow_mut().clear();
                // The critical one: release the active-swipe latch so
                // taps work again immediately.
                SWIPE_GESTURE_ACTIVE
                    .store(false, std::sync::atomic::Ordering::Relaxed);
                // Bump the tap generation so any in-flight suppressed
                // tap from this gesture can't resurrect.
                TAP_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // A cancelled gesture produces no dispatch, so any
                // focus-loss signal accumulated during it has no
                // dispatch to abort. Clear it so it can't leak into
                // the NEXT gesture's drag_end. (drag_end's normal
                // dispatch path consumes the flag via take_*; cancel
                // skips that, leaving the flag stuck until something
                // else clears it.)
                clear_swipe_interrupted_by_focus_loss();
            });
        }

        // The controller stays on overlay (not keyboard_box) so
        // touches in the overshoot margin still register. Coordinate
        // translation from overlay frame → keyboard_box frame happens
        // inside drag_begin / drag_update, using the kb_offset
        // captured at drag_begin via compute_bounds. The trail widget
        // continues to receive raw overlay-frame coords because the
        // trail draws in overlay-frame too.
        self.overlay.add_controller(drag);
    }

    /// Subscribe to engine events on a 30Hz GLib timeout so we can safely
    /// touch widgets from the main loop. When the keypad state changes
    /// across "page" boundaries (Normal/Shifted ↔ Symbols ↔ Numbers ↔
    /// Emoji), we swap the active layout and call `rebuild` — without that,
    /// pressing the symbols toggle just updated the cell silently and the
    /// keys never changed on screen.
    fn attach_engine_listener(&self) {
        let rx = self.engine.subscribe();
        let widgets = self.key_widgets.clone();
        let current_layout = self.current_layout.clone();
        let keyboard_box = self.keyboard_box.clone();
        let engine = self.engine.clone();
        let recognizer = self.recognizer.clone();
        let _cfg_letter_name = self.cfg.layout.active.clone();
        let cfg = self.cfg.clone();
        let trail_for_settings = self.trail.clone();

        // Remember which letter layout to return to when leaving Symbols/
        // Numbers/Emoji. Defaults to whatever the config says (resolved
        // through the orthogonal Letters/Geometry/Cardinal axes).
        // This is a local closure-scoped Rc; the SymbolsToggle path
        // is the only consumer. The arrows-tray path uses the
        // struct field `pre_tray_layout` (separate stash, different
        // semantics).
        let last_letter_layout = Rc::new(RefCell::new(resolve_letter_layout(&cfg)));

        // Microterm handle for the event pump (steno-exit hides the
        // suggestion ribbon through this).
        let microterm_handle = self.microterm.clone();

        // Engine→view event drain. Self-adaptive cadence: snap to
        // FAST_MS while events keep arriving (responsive typing), back
        // off toward IDLE_MS when the channel runs dry (cuts always-on
        // wakeups on phone, where each periodic timer competes with
        // GTK's frame clock for the main thread). Same pattern as
        // spawn_engine_to_vkbd_bridge — see its comments for rationale.
        const FAST_MS: u64 = 100;
        const IDLE_MS: u64 = 500;
        let interval_slot: Rc<std::cell::Cell<u64>> =
            Rc::new(std::cell::Cell::new(IDLE_MS));
        type Rearm = Rc<dyn Fn()>;
        let rearm_slot: Rc<RefCell<Option<Rearm>>> = Rc::new(RefCell::new(None));
        let rearm_inner = rearm_slot.clone();

        // Drain body — exactly the original match-on-events. Wrapped in
        // an Rc<Fn()> so we can self-reschedule with a possibly-different
        // interval based on whether we drained anything.
        let drain: Rearm = Rc::new(move || {
            let mut drained = 0u32;
            while let Ok(ev) = rx.try_recv() {
                drained += 1;
                match ev {
                    EngineEvent::StateChanged(new_state) => {
                        debug!("State changed -> {:?}", new_state);
                        let cur_name = current_layout.borrow().name.clone();
                        // PRIMARY vs TEMPORARY layout model
                        // --------------------------------
                        // Primary = the user's persistent choice in
                        // settings (qwerty / dvorak / cardinal /
                        // stenotype) — stored on disk.
                        // Temporary = transient pages reached from
                        // within the session: symbols, numbers, emoji,
                        // arrows. NEVER persists.
                        //
                        // The same return-to-letters logic applies to
                        // ALL primaries, including stenotype: tap ABC
                        // in steno → symbols page (transient); tap ABC
                        // again → back to steno (last_letter_layout
                        // holds it). A previous version hijacked this
                        // transition to PERSISTENTLY exit steno —
                        // saving experimental_mode='none' to disk just
                        // because the user wanted symbols. That was the
                        // persistence leak the user reported.
                        //
                        // While in steno, ignore state changes that
                        // are purely letter-shift flips (Normal /
                        // Shifted / CapsLock from auto-cap) — steno
                        // doesn't render shift pages, there's nothing
                        // to rebuild. But transient-page transitions
                        // (Symbols / Numbers / Emoji) MUST pass through
                        // to the regular handler so the user can step
                        // out to symbols and back to their steno
                        // primary.
                        if cur_name == "stenotype"
                            && matches!(
                                new_state,
                                KeypadState::Normal
                                    | KeypadState::Shifted
                                    | KeypadState::CapsLock
                            )
                        {
                            continue;
                        }
                        match new_state {
                            KeypadState::Symbols => {
                                if cur_name != "symbols" {
                                    // Only stash the previous layout if it was
                                    // a letter layout. Coming from "numbers"
                                    // (the second symbols page) we want to
                                    // preserve last_letter_layout pointing at
                                    // the original letters, not overwrite it
                                    // with the numbers page.
                                    if cur_name != "numbers" {
                                        *last_letter_layout.borrow_mut() =
                                            current_layout.borrow().clone();
                                        // Leaving the steno primary
                                        // for a transient page: clear
                                        // the phoneme buffer so a
                                        // stale partial stroke can't
                                        // pair with a fresh stroke on
                                        // return. Cheap and safe — the
                                        // buffer is only meaningful in
                                        // stenotype, no-op elsewhere.
                                        if cur_name == "stenotype" {
                                            crate::steno::global().clear_buffer();
                                            microterm_handle.hide();
                                        }
                                    }
                                    // Choose the symbols variant
                                    // based on the source layout's
                                    // geometry. Ortho letter layouts
                                    // (cardinal, stenotype) deserve
                                    // the matching ortho symbols
                                    // page — without this, the
                                    // standard symbols page renders
                                    // staggered (Row 3/4 have 1.5
                                    // and 4.0-wide keys mixed with
                                    // 1.0-wide ones) and the visual
                                    // continuity is broken.
                                    let was_ortho = last_letter_layout
                                        .borrow()
                                        .ortholinear;
                                    *current_layout.borrow_mut() = if was_ortho {
                                        Layout::default_symbols_ortho()
                                    } else {
                                        Layout::default_symbols()
                                    };
                                    rebuild_view(
                                        &keyboard_box,
                                        &widgets,
                                        &current_layout,
                                        &engine,
                                        &recognizer,
                                    );
                                }
                            }
                            KeypadState::Numbers => {
                                if cur_name != "numbers" {
                                    // Same logic — don't overwrite the
                                    // letter stash when we're already on the
                                    // symbols page.
                                    if cur_name != "symbols" {
                                        *last_letter_layout.borrow_mut() =
                                            current_layout.borrow().clone();
                                        if cur_name == "stenotype" {
                                            crate::steno::global().clear_buffer();
                                            microterm_handle.hide();
                                        }
                                    }
                                    let was_ortho_n = last_letter_layout
                                        .borrow()
                                        .ortholinear;
                                    *current_layout.borrow_mut() = if was_ortho_n {
                                        Layout::default_numbers_ortho()
                                    } else {
                                        Layout::default_numbers()
                                    };
                                    rebuild_view(
                                        &keyboard_box,
                                        &widgets,
                                        &current_layout,
                                        &engine,
                                        &recognizer,
                                    );
                                }
                            }
                            KeypadState::Normal | KeypadState::Shifted | KeypadState::CapsLock => {
                                // "Letter family" = any layout that is NOT a
                                // transient symbols/numbers/emoji/arrows page.
                                // This includes qwerty, dvorak, AND the
                                // cardinal+stenotype primaries — the cardinal
                                // and steno cases used to be wrong (rebuilt
                                // full layout on every auto-cap shift), but
                                // the negative check below correctly leaves
                                // them alone when not transient.
                                //
                                // Classification lives on Layout itself
                                // (`is_transient_page`) so adding a new
                                // transient layer only updates one place.
                                let is_transient_page =
                                    current_layout.borrow().is_transient_page();
                                if is_transient_page {
                                    // Coming back from Symbols/Numbers/Emoji
                                    // to the user's primary. last_letter_layout
                                    // holds whatever primary the user was on
                                    // BEFORE going transient — qwerty, dvorak,
                                    // cardinal, OR stenotype, all handled the
                                    // same way.
                                    *current_layout.borrow_mut() = last_letter_layout.borrow().clone();
                                    rebuild_view(
                                        &keyboard_box,
                                        &widgets,
                                        &current_layout,
                                        &engine,
                                        &recognizer,
                                    );
                                } else {
                                    // Already on a letter-family layout — just
                                    // a shift toggle. Update labels in place
                                    // (no rebuild, no flicker).
                                    for w in widgets.borrow().iter() {
                                        w.update_for_state(new_state);
                                    }
                                }
                            }
                            KeypadState::Emoji => {
                                // No emoji layout exists yet. If an emoji key
                                // is ever added and fires this, DON'T leave the
                                // user trapped in a state with no visible page
                                // and no way back — fall back to the letter
                                // layout and reset the engine state so the next
                                // tap behaves normally. (When a real emoji
                                // page lands, replace this with the page swap.)
                                tracing::warn!(
                                    "StateChanged(Emoji) but no emoji layout — \
                                     restoring letters to avoid a dead-end"
                                );
                                engine.set_keypad_state(KeypadState::Normal);
                                *current_layout.borrow_mut() =
                                    last_letter_layout.borrow().clone();
                                rebuild_view(
                                    &keyboard_box,
                                    &widgets,
                                    &current_layout,
                                    &engine,
                                    &recognizer,
                                );
                            }
                            KeypadState::Voice => {
                                // Voice recording is an overlay (microterm),
                                // not a layout swap — the keys stay visible
                                // underneath. Deliberate no-op here; the voice
                                // bridge drives the recording UI.
                            }
                        }
                    }
                    EngineEvent::TextCommitted(text) => {
                        // Per-character debug! (was info!) — fires on
                        // every keystroke / swipe / voice / LLM commit.
                        // The string formatting churn at info! level
                        // was a measurable perf hit on phone; the
                        // engine→vkbd bridge already logs commits at
                        // the point text actually reaches the host.
                        debug!("Committed text: {:?}", text);
                    }
                    EngineEvent::KeyAction(action) => {
                        debug!("Key action: {:?}", action);
                        if matches!(action, crate::keyboard::KeyAction::CodeLangCycle) {
                            // Cycle the process-wide CodeMode atomic
                            // (Prose → C → Rust → Python → Prose) and
                            // rebuild the layout. The layout function
                            // reads the atomic fresh during rebuild,
                            // so the new word/template cells appear.
                            // Only useful on the cardinal_grid_words
                            // layout — on any other layout the cycle
                            // happens silently and the change takes
                            // effect when the user switches to it.
                            let new_mode = crate::keyboard::code_mode::cycle();
                            info!(
                                "CodeLangCycle: cycled to {:?} ({})",
                                new_mode,
                                new_mode.short_label()
                            );
                            let cur_name = current_layout.borrow().name.clone();
                            if cur_name == "cardinal_grid_words" {
                                // Re-resolve the layout from the
                                // factory so the cluster cells reflect
                                // the new mode. Width/height stays the
                                // same so no resize jank.
                                *current_layout.borrow_mut() =
                                    crate::layouts::Layout::default_cardinal_grid_words();
                                rebuild_view(
                                    &keyboard_box,
                                    &widgets,
                                    &current_layout,
                                    &engine,
                                    &recognizer,
                                );
                            }
                        }
                        if matches!(action, crate::keyboard::KeyAction::Settings) {
                            // Hide any lingering microterm content
                            // (steno ribbon, voice/LLM partials, toasts)
                            // before opening settings — see
                            // show_settings_panel's cancel call. The
                            // bridges hide their own content on cancel,
                            // but the steno suggestion path lingers
                            // for 5 s on its own timer; this explicit
                            // hide closes that gap.
                            microterm_handle.hide();
                            let fresh_cfg = crate::config::Config::load_or_default()
                                .unwrap_or_else(|e| {
                                    tracing::warn!(
                                        "settings reopen: load_or_default failed: {}; \
                                         using captured snapshot",
                                        e
                                    );
                                    cfg.clone()
                                });
                            show_settings_panel(
                                &keyboard_box,
                                &current_layout,
                                &widgets,
                                &engine,
                                &recognizer,
                                &fresh_cfg,
                                &trail_for_settings,
                                &last_letter_layout,
                            );
                        }
                        if matches!(action, crate::keyboard::KeyAction::ShowFullKeyboard) {
                            // Swap arrows tray → full keyboard.
                            let cur_name = current_layout.borrow().name.clone();
                            if cur_name == "arrows" {
                                *current_layout.borrow_mut() =
                                    last_letter_layout.borrow().clone();
                                // Sync engine state to match the
                                // visible layout (Normal for the
                                // letter primaries). Without this,
                                // engine could be in a stale Symbols
                                // state — making the next ?123 tap
                                // a no-op because engine thinks
                                // we're already on symbols.
                                engine.set_keypad_state(
                                    current_layout
                                        .borrow()
                                        .corresponding_keypad_state(),
                                );
                                rebuild_view(
                                    &keyboard_box,
                                    &widgets,
                                    &current_layout,
                                    &engine,
                                    &recognizer,
                                );
                                info!("show full keyboard: swapped from arrows tray");
                            }
                        }
                        if matches!(action, crate::keyboard::KeyAction::HideKeyboard) {
                            // Tap on the ✕ button (in arrows tray or
                            // anywhere). Hide the window — the user
                            // can re-summon it by tapping any text
                            // field. Without this handler, the engine
                            // emits a HideKeyboard event but no UI
                            // code listened for it, so the button
                            // tapped silently. (vkbd bridge handler
                            // had a `_ => {}` catch-all that ate it.)
                            if let Some(root) = keyboard_box.root() {
                                if let Some(win) = root.dynamic_cast_ref::<gtk4::Window>() {
                                    info!("HideKeyboard action: hiding window");
                                    win.set_visible(false);
                                }
                            }
                        }
                    }
                    EngineEvent::VoiceTranscript { text, partial } => {
                        info!(
                            "Voice {}: {}",
                            if partial { "partial" } else { "final" },
                            text
                        );
                    }
                    EngineEvent::ModifierPressed(_) => {}
                    EngineEvent::KeyLongPress(_) => {
                        // Long-press is handled by other subscribers
                        // (e.g. the voice bridge toggles the microterm
                        // status display). The keyboard_view itself
                        // doesn't react — popover surfacing is on the
                        // KeyWidget directly and predates this event.
                    }
                    EngineEvent::LlmRequest(_) => {
                        // LLM dispatch is owned by the LLM bridge.
                    }
                }
            }
            // Adapt the next interval: snap to FAST_MS if anything
            // arrived this tick; otherwise double the interval up to
            // IDLE_MS. The cap keeps first-event-after-idle latency
            // bounded at one IDLE_MS — 500ms here, which is fine since
            // StateChanged/KeyLongPress/VoiceTranscript are infrequent
            // user-initiated events and a half-second worst-case
            // pickup-from-idle is imperceptible.
            let next = if drained > 0 {
                FAST_MS
            } else {
                (interval_slot.get().saturating_mul(2)).min(IDLE_MS)
            };
            interval_slot.set(next);
            if let Some(rearm) = rearm_inner.borrow().as_ref() {
                let rearm = rearm.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(next),
                    move || rearm(),
                );
            }
        });
        *rearm_slot.borrow_mut() = Some(drain.clone());
        drain();
    }
}

/// Resolve a layout name from config to a Layout instance. Defaults to
/// QWERTY for any unknown name.
/// Resolve a `Layout` from the user's orthogonal preference axes:
/// `letters` (qwerty|dvorak), `row_style` (staggered|ortholinear), and
/// `cardinal_enabled` (replaces both above with a 3x3 cardinal grid).
///
/// This replaces the older single-name lookup (which fused all three
/// axes into one string like "en_us_dvorak_ortho") so users can mix
/// and match — e.g. "Dvorak letters in staggered geometry".
fn resolve_letter_layout(cfg: &crate::config::Config) -> Layout {
    // `anchor` is only consumed by the cardinal experimental branch
    // below; gate it so a basic build (no input-modes) doesn't warn
    // about an unused binding.
    #[cfg(feature = "input-modes")]
    let anchor = cfg
        .layout
        .thumb_arc
        .as_ref()
        .map(|a| a.anchor)
        .unwrap_or(crate::config::ThumbAnchor::BottomRight);

    // Experimental modes layer on top of the base letter system.
    // They take priority over the standard letter layout — but ONLY
    // when compiled in. A basic build (no `input-modes`) ignores
    // experimental_mode entirely and always resolves base letters,
    // so a config carried over from a fuller build degrades
    // gracefully instead of selecting an unavailable surface.
    #[cfg(feature = "input-modes")]
    match cfg.layout.experimental_mode.as_str() {
        "stenotype" => {
            // Fixed single-row layout; ignores ortho/number-row.
            return Layout::default_stenotype();
        }
        "cardinal" => {
            // Cardinal grid: pick the variant matching the base
            // letter system so swiping muscle memory carries over.
            // The `letters` field is qwerty|dvorak here (never
            // "cardinal"/"stenotype" thanks to load-time sanitize).
            if cfg.layout.letters == "dvorak" {
                return Layout::default_cardinal_grid_dvorak_for_anchor(anchor);
            }
            return Layout::default_cardinal_grid();
        }
        "cardinal_words" => {
            // 5-column cardinal grid with prose/code word columns.
            // Always uses the qwerty-derived letter set (a/n/i/t/s/c/h/r)
            // regardless of cfg.layout.letters — the rightmost 2 columns
            // are the differentiator and they're language-independent.
            return Layout::default_cardinal_grid_words();
        }
        _ => {} // "none" → fall through to base letter layout
    }

    let ortho = cfg.layout.row_style == "ortholinear";
    let mut layout = match cfg.layout.letters.as_str() {
        "dvorak" => {
            if ortho {
                Layout::default_dvorak_ortholinear()
            } else {
                Layout::default_dvorak_staggered()
            }
        }
        _ => {
            if ortho {
                Layout::default_qwerty_ortho()
            } else {
                Layout::default_qwerty()
            }
        }
    };

    // Prepend a number row when enabled. The row has 10 keys (1-9 then 0)
    // each at width=1.0, matching the 10-column grid that letter layouts
    // assume. Adds one row of height; the desired-height logic
    // automatically scales the keyboard window down to fit.
    if cfg.layout.number_row {
        let mut number_keys = Vec::with_capacity(10);
        for c in "1234567890".chars() {
            number_keys.push(crate::keyboard::Key::character(&c.to_string()));
        }
        let number_row = crate::layouts::Row {
            keys: number_keys,
            weight: 0.85,
            offset: 0.0,
        };
        layout.rows.insert(0, number_row);
    }

    layout
}

/// Back-compat shim — old call sites used `default_letter_layout(name)`
/// with a single string. New code should use `resolve_letter_layout`
/// which understands the orthogonal axes. We keep this helper for
/// places that still have a layout name handy (e.g. when loading a
/// custom JSON layout — not currently exercised but kept for the
/// future).
#[allow(dead_code)]
fn default_letter_layout(name: &str) -> Layout {
    match name {
        "en_us_dvorak_ortho" | "en_us_dvorak" => Layout::default_dvorak_ortholinear(),
        "cardinal_grid_en" => Layout::default_cardinal_grid(),
        "cardinal_grid_dvorak" => Layout::default_cardinal_grid_dvorak(),
        "cardinal_grid_words" => Layout::default_cardinal_grid_words(),
        _ => Layout::default_qwerty(),
    }
}

/// Tray transition logic, callable from inside the swipe-handler
/// closure (which can't hold a `&KeyboardView`). Same shape as
/// `KeyboardView::enter_tray` but works on cloned `Rc`s directly.
///
/// Tears down the current keyboard contents, replaces with a fresh
/// `MinimizedTray`. The tray's own callbacks (wired up here) restore
/// the full keyboard on expand-tap or hide it on close-tap.
fn enter_tray_inline(
    visible_mode: &Rc<Cell<VisibleMode>>,
    kb_box: &gtk4::Box,
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    current_layout: &Rc<RefCell<Layout>>,
    engine: &KeyboardEngine,
    recognizer: &Rc<RefCell<GestureRecognizer>>,
) {
    if visible_mode.get() == VisibleMode::Tray {
        return;
    }
    visible_mode.set(VisibleMode::Tray);

    // Clear stranded swipe state: the minimize-fling path is the
    // only drag_end branch that wasn't clearing SWIPE_GESTURE_ACTIVE
    // (other branches all do, via either dispatch or the
    // focus-loss-abort handler). Without this, the next tap after
    // the user re-expands the tray sees the latch still true and
    // gets suppressed by the click handler's tap-eviction check.
    // Same idea for the focus-loss flag — a swipe that ended in
    // minimize is a complete gesture; whatever the focus state
    // was, it's not relevant to the next gesture.
    SWIPE_GESTURE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    TAP_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    clear_swipe_interrupted_by_focus_loss();

    while let Some(child) = kb_box.first_child() {
        kb_box.remove(&child);
    }
    widgets.borrow_mut().clear();

    // Determine the tray's dominant side from the active thumb-arc.
    // When single-hand mode is on (right or left), the tray bar
    // should sit on the same side. When off, default to right.
    let side = match CURRENT_THUMB_ARC.lock().as_ref().map(|a| a.anchor) {
        Some(crate::config::ThumbAnchor::BottomLeft) => crate::ui::DominantSide::Left,
        _ => crate::ui::DominantSide::Right,
    };

    let visible_mode_ex = visible_mode.clone();
    let kb_box_ex = kb_box.clone();
    let widgets_ex = widgets.clone();
    let cur_layout_ex = current_layout.clone();
    let engine_ex = engine.clone();
    let recognizer_ex = recognizer.clone();
    let on_expand: Rc<dyn Fn()> = Rc::new(move || {
        info!("tray (inline): expand → Full");
        visible_mode_ex.set(VisibleMode::Full);
        while let Some(child) = kb_box_ex.first_child() {
            kb_box_ex.remove(&child);
        }
        widgets_ex.borrow_mut().clear();
        rebuild_view(&kb_box_ex, &widgets_ex, &cur_layout_ex, &engine_ex, &recognizer_ex);
    });

    let visible_mode_cl = visible_mode.clone();
    let kb_box_cl = kb_box.clone();
    let on_close: Rc<dyn Fn()> = Rc::new(move || {
        info!("tray (inline): close → Dismissed");
        visible_mode_cl.set(VisibleMode::Dismissed);
        while let Some(child) = kb_box_cl.first_child() {
            kb_box_cl.remove(&child);
        }
        if let Some(root) = kb_box_cl.root() {
            if let Some(win) = root.dynamic_cast_ref::<gtk4::Window>() {
                win.set_visible(false);
            }
        }
    });

    let tray = crate::ui::MinimizedTray::new(
        side,
        engine.clone(),
        crate::ui::minimized_tray::TrayCallbacks {
            on_expand,
            on_close,
        },
    );
    kb_box.append(tray.widget());
}

/// Tear down all key widgets in `keyboard_box` and rebuild from
/// `current_layout`. Used when the user toggles between letter /
/// symbol / number pages, when settings change, or when the keyboard
/// switches between Full and Tray and back.
///
/// We have to recreate widgets (rather than just updating labels)
/// because the layouts have different row counts, key widths, and
/// key actions.
fn rebuild_view(
    keyboard_box: &gtk4::Box,
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    current_layout: &Rc<RefCell<Layout>>,
    engine: &KeyboardEngine,
    _recognizer: &Rc<RefCell<GestureRecognizer>>,
) {
    // Invalidate the hitbox cache — refresh_hitboxes will see the
    // bumped generation on the next drag_begin and recompute.
    // Without this, the cache would serve stale widget bounds and
    // touch-to-key hit testing would be wrong after a layout change.
    HITBOX_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    while let Some(child) = keyboard_box.first_child() {
        keyboard_box.remove(&child);
    }
    widgets.borrow_mut().clear();

    let mut layout = current_layout.borrow().clone();
    let state = engine.keypad_state();

    // Guarantee a settings (⚙) key exists in every layout, regardless of
    // what the JSON/default definition included. Some layouts (Dvorak,
    // imported JSON) don't have one; without it the user has no way to
    // get back to the settings panel and switch layouts. We inject one
    // into the last row if no Settings action is present anywhere.
    //
    // This is a UX safety net — better than relying on every layout
    // author to remember the settings button. Defined at view-build time
    // rather than load time so we don't mutate the user's saved layout.
    //
    // Sizing: layouts are typically authored to sum exactly to
    // `grid_columns`. Adding a width=1.0 key would overflow the row and
    // push keys off-screen (this was the bug on Dvorak: 10 cells + 1 ⚙
    // = 11 cells in a 10-column grid). To keep the row at 10 cells, we
    // shrink whichever existing key is the widest (typically space-bar
    // at width 4.0+) by 1.0 to make room. This preserves total width.
    let has_settings = layout.rows.iter().any(|row| {
        row.keys
            .iter()
            .any(|k| matches!(k.action, KeyAction::Settings))
    });
    if !has_settings {
        if let Some(last_row) = layout.rows.last_mut() {
            // Find widest key; shrink it by 1.0 (but never below 1.0).
            let widest_idx = last_row
                .keys
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.width.partial_cmp(&b.width).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i);

            if let Some(idx) = widest_idx {
                if last_row.keys[idx].width >= 2.0 {
                    let old_w = last_row.keys[idx].width;
                    last_row.keys[idx].width = old_w - 1.0;
                    tracing::debug!(
                        "Layout '{}' missing Settings key; shrinking '{}' from {} to {} units to make room for ⚙",
                        layout.name,
                        last_row.keys[idx].label,
                        old_w,
                        last_row.keys[idx].width
                    );
                } else {
                    tracing::debug!(
                        "Layout '{}' missing Settings key; no key wider than 2.0 to shrink — row may overflow",
                        layout.name
                    );
                }
            }

            last_row.keys.push(
                Key::modifier("⚙", KeyAction::Settings).with_width(1.0),
            );
        }
    }

    // Compute logical px per unit-column from the actual screen size.
    // The keyboard spans the full screen width (layer-shell anchors
    // Left+Right). Each row has at most `layout.grid_columns` units;
    // we want all keys to fit including spacing between them.
    //
    // Formula: `screen_w = unit_w * grid_columns + spacing * (grid_columns - 1) + margins`
    // Solve for unit_w with a small safety margin so we don't overflow
    // on rounding errors.
    let unit_w_default = compute_unit_width(
        keyboard_box,
        layout.grid_columns,
        &layout.rows,
        layout.ortholinear,
    );

    // Surface width — needed for per-row arc-fitting math. This must
    // be the keyboard_box's CONTENT area, not the screen width,
    // because the polygon's x ∈ [0, 1] maps to "left edge of where
    // keys can render" → "right edge of where keys can render". The
    // keyboard_box has 20px outer margins on each side (set in
    // KeyboardView::new), so the content area is 40px narrower than
    // the screen.
    //
    // Past bug: surface_w was set to the monitor width directly. The
    // polygon then mapped (1.0, ...) to a screen-edge pixel, but the
    // keyboard_box's outer margin pushed everything 16px right, so
    // the rightmost keys rendered off the right edge of the screen
    // in single-hand mode. Matches keyboard_box's actual margins
    // (margin_start=8 + margin_end=8 = 16, see KeyboardView::new).
    const OUTER_MARGIN_TOTAL: i32 = 8 * 2;
    let surface_w = keyboard_box.width().max(0) as f64;
    let surface_w = if surface_w > 32.0 {
        // Already an allocated width — that's the content area
        // already, GTK reports `width()` as the box's inner-content
        // width modulo padding (but we don't set padding on
        // keyboard_box, just margins, and margins aren't part of
        // .width()). So this branch is correct as-is.
        surface_w
    } else {
        // First-build fallback: read from the monitor and subtract
        // our known outer margin total, since the keyboard_box hasn't
        // been allocated yet and will have margins applied.
        let monitor_w = monitor_width_logical().unwrap_or(360.0);
        (monitor_w - OUTER_MARGIN_TOTAL as f64).max(100.0)
    };
    // Scale-aware cap shared by surface_w (row geometry), the
    // keyboard_box pin (rendered width), and compute_unit_width (key
    // sizing). All three MUST agree or the layout disagrees with
    // itself and keys overflow or clip.
    let scaled_cap = {
        let s = (WIDTH_SCALE.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1000.0)
            .clamp(0.5, 2.0);
        (MAX_KEYBOARD_WIDTH as f64 * s).min(1100.0)
    };

    // Cap the surface used for row geometry to match the capped
    // unit-width. On a wide display the keyboard_box is pinned to the
    // (scale-aware) cap and centered (see below); the row geometry
    // must use the same width or rows get laid out for the full
    // monitor while keys are sized for the cap.
    let surface_w = surface_w.min(scaled_cap - OUTER_MARGIN_TOTAL as f64);

    let row_count = layout.rows.len();

    // Wide-display handling. surface_w is already clamped to the max
    // (via compute_unit_width's MAX_KEYBOARD_WIDTH), but the
    // keyboard_box itself is hexpand(true) and will stretch to the
    // full layer-shell width on a laptop/monitor, distributing the
    // extra space and pushing the rightmost keys off-screen. Detect
    // the wide case by comparing the actual allocation to the cap:
    // if the box is wider than the capped surface, switch it to a
    // fixed, centered width so the rows render at phone proportions
    // in the middle of the screen. On a narrow (phone) display the
    // allocation ≈ surface_w, so we leave it hexpanding/full-width.
    // Scale-aware cap (shared with surface_w + compute_unit_width
    // above). max_kb_w_f is just the f64 form for the comparison.
    let max_kb_w_f = scaled_cap;
    // Prefer the live allocation; before first allocation it's 0, so
    // fall back to the monitor width — otherwise the very first paint
    // on a wide display would stretch full-width until a later
    // rebuild corrects it.
    let alloc_w = {
        let a = keyboard_box.width().max(0) as f64;
        if a > 32.0 {
            a
        } else {
            monitor_width_logical().unwrap_or(360.0)
        }
    };
    if alloc_w > max_kb_w_f + 1.0 {
        // Wide display: pin to the (scaled) cap and center.
        keyboard_box.set_hexpand(false);
        keyboard_box.set_halign(gtk4::Align::Center);
        keyboard_box.set_width_request(scaled_cap as i32);
    } else {
        // Narrow display (phone) or not-yet-allocated: fill the
        // screen as before.
        keyboard_box.set_hexpand(true);
        keyboard_box.set_halign(gtk4::Align::Fill);
        keyboard_box.set_width_request(-1);
    }

    for (i, row) in layout.rows.iter().enumerate() {
        // Per-row geometry. Without single-hand mode, every row is
        // full-width and gets `unit_w_default`. With single-hand mode,
        // each row's width is the polygon's x-range at the row's
        // vertical center; unit_w shrinks correspondingly.
        let row_y_norm = if row_count > 1 {
            (i as f64 + 0.5) / row_count as f64
        } else {
            0.5
        };
        let geom = compute_row_geometry(
            &layout,
            row,
            row_y_norm,
            surface_w,
            unit_w_default as f64,
        );

        let row_box = build_row_static_v2(row, state, &layout, engine, widgets, &geom);
        keyboard_box.append(&row_box);
    }

    info!(
        "View rebuilt for state={:?}, layout={}, ortho={}, unit_w_default={}",
        state, layout.name, layout.ortholinear, unit_w_default
    );

    // Play the page-settle transition: add the from-state class
    // (faded + nudged up) now, then drop it on the next main-loop
    // iteration so the CSS transition eases the view back to rest.
    // GPU-cheap (opacity + transform only); no layout reflow.
    // `timeout_add_local_once` with a 0ms delay defers to the next
    // loop tick, which is enough for GTK to apply the from-state
    // before the to-state, so the transition actually plays.
    keyboard_box.add_css_class("kb-rebuilding");
    {
        let kb = keyboard_box.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(0), move || {
            kb.remove_css_class("kb-rebuilding");
        });
    }

    // Match the surface height to what the compact-aware layout
    // computation thinks we need RIGHT NOW for THIS layout, not
    // whatever was last stored in DESIRED_KEYBOARD_HEIGHT (which
    // would be stale if we just transitioned between layouts of
    // different row counts — e.g., stenotype 2-row → symbols 4-row).
    // Reading from the live atomic flags + the current layout's
    // row count means every rebuild path is correctly sized without
    // each call site having to remember to update the atomic before
    // calling rebuild_view.
    //
    // Store back into DESIRED_KEYBOARD_HEIGHT so placement.rs and
    // any other consumer that reads it post-rebuild gets the
    // current value.
    let row_count = layout.rows.len();
    let desired_h = desired_height_for_current_layout_named(row_count, &layout.name);
    DESIRED_KEYBOARD_HEIGHT.store(desired_h, std::sync::atomic::Ordering::Relaxed);
    if let Some(root) = keyboard_box.root() {
        if let Some(win) = root.dynamic_cast_ref::<gtk4::Window>() {
            // Always issue the set_default_height request — even when
            // `default_height()` reports the same value we'd ask for.
            // The cached `default_height()` is the LAST REQUESTED
            // value, not what the compositor actually allocated. On
            // some layer-shell setups (KWin on Plasma Mobile being
            // the known one), a request for 107px can be ignored
            // because the compositor was busy / didn't pick it up
            // / clamped to some minimum. If we then check
            // `default_height() == 107` and skip the update, the
            // window stays at the old (taller) size forever. By
            // re-issuing on every rebuild, the request goes out
            // again and the compositor gets another chance to
            // honor it.
            //
            // GTK collapses no-op resizes internally, so this is
            // free in the common case where the value didn't
            // change.
            let current_h = win.default_height();
            debug!(
                "rebuild_view: requesting window height {} (was {}, rows={})",
                desired_h, current_h, row_count
            );
            win.set_default_height(desired_h);
        }
    }
}

/// Geometry parameters for rendering a single row.
struct RowGeometry {
    /// Leading padding before the first key, in logical px.
    leading_pad_px: f64,
    /// Width per unit-column inside this row, in logical px.
    unit_w_px: f64,
}

/// Compute the (leading-padding, unit-width) pair for a row, taking
/// thumb-arc compaction into account when active.
///
/// Without compaction (CURRENT_THUMB_ARC is None): unit_w = the default;
/// leading pad is just `row.offset * unit_w` (the existing staggered-row
/// behavior).
///
/// With compaction: the arc polygon's x-range at the row's vertical
/// center determines the row's left and right edges. unit_w is then the
/// available width / row's total units. The polygon is in normalized
/// [0,1] coords across the surface, so we scale by `surface_w` to get
/// pixels.
fn compute_row_geometry(
    layout: &Layout,
    row: &crate::layouts::Row,
    row_y_norm: f64,
    surface_w: f64,
    default_unit_w: f64,
) -> RowGeometry {
    // Compute row's total key units (sum of widths).
    let total_units: f64 = row.keys.iter().map(|k| k.width as f64).sum();
    if total_units <= 0.0 {
        return RowGeometry {
            leading_pad_px: 0.0,
            unit_w_px: default_unit_w,
        };
    }

    // Take a snapshot of the current arc. We drop the lock immediately
    // so the rest of this function can't deadlock — polygon_x_range
    // only needs the polygon, not the live mutex.
    let arc_snapshot: Option<crate::config::ThumbArc> = {
        CURRENT_THUMB_ARC.lock().clone()
    };

    if let Some(arc) = arc_snapshot {
        // Single-hand mode active. Compute the x-range of the arc
        // polygon at this row's vertical center.
        if let Some((x_min_norm_raw, x_max_norm_raw)) = polygon_x_range(&arc.polygon, row_y_norm) {
            // Per-row CRUNCH: the polygon shape gives us a thumb-
            // friendly wedge (narrow top, wide bottom) but the middle
            // and bottom LETTER rows still feel too spread out — the
            // thumb has to travel a lot laterally between adjacent
            // keys. Apply an additional shrink factor that peaks on
            // the middle row and tapers off for the top (already
            // narrow) and the spacebar row (needs width for arrow
            // swipes).
            //
            // Crunch happens ANCHORED to the dominant-thumb side:
            // for a right-handed user the right edge stays put and
            // the LEFT edge moves rightward; for left-handed, mirror.
            let crunch = middle_row_crunch_factor(row_y_norm);
            let (x_min_norm, x_max_norm) =
                apply_crunch_anchored(x_min_norm_raw, x_max_norm_raw, crunch, arc.anchor);
            // Map [0, 1] polygon coords to pixel coords on the
            // keyboard_box. Note: keyboard_box.width() reports the
            // OUTER content area (margins not included), but the
            // .keyboard-view CSS has padding:4px which shrinks the
            // actual key-rendering area by 4px each side.
            //
            // Clamp the polygon edges inward by that padding so
            // rows can't render past the CSS-padded boundary.
            const KB_VIEW_PADDING_PX: f64 = 4.0;
            let raw_left = x_min_norm * surface_w;
            let raw_right = x_max_norm * surface_w;
            let row_left_px = raw_left.max(KB_VIEW_PADDING_PX);
            let row_right_px =
                raw_right.min(surface_w - KB_VIEW_PADDING_PX);
            let row_width_px = (row_right_px - row_left_px).max(0.0);

            // The row_box has `spacing: 4` between keys, so for an
            // N-key row, (N-1) * 4 pixels of the available width go
            // to spacing — NOT to keys. We have to subtract that
            // before dividing by total_units, otherwise the keys
            // render wider than the polygon's allotted region and
            // the row overflows past the polygon's right edge.
            //
            // Past bug: the unit_w was row_width_px / total_units,
            // ignoring spacing. For a 3-key row at width 216, that
            // gave unit_w=72px and total render = 3*72 + 2*4 = 224px,
            // over budget. Compounded by single-hand mode being
            // applied at the SCREEN edge, those overflow px push keys
            // off the right side of the screen.
            //
            // Account for full per-key CSS chrome: border (1px each
            // side = 2px) + padding (2px each side = 4px) = 6px per
            // key. Past bug had this at 2px (border alone), which
            // under-budgeted by ~40px on a 10-column row — that's
            // a full key's worth and caused the rightmost column to
            // render off-screen.
            const SPACING_PX: f64 = 4.0;
            const BORDER_PER_KEY_PX: f64 = 6.0;
            let key_count = row.keys.len() as f64;
            let spacing_total = SPACING_PX * (key_count - 1.0).max(0.0);
            let border_total = BORDER_PER_KEY_PX * key_count;
            let usable_width = (row_width_px - spacing_total - border_total).max(0.0);
            let unit_w_px = usable_width / total_units;

            tracing::debug!(
                "thumb-arc row y={:.2}: x_norm=[{:.2}, {:.2}] surface_w={:.0} → \
                 left={:.0}px right={:.0}px row_w={:.0}px - spacing={:.0}px - \
                 border={:.0}px → unit_w={:.1}px (total_units={})",
                row_y_norm,
                x_min_norm,
                x_max_norm,
                surface_w,
                row_left_px,
                row_right_px,
                row_width_px,
                spacing_total,
                border_total,
                unit_w_px,
                total_units
            );

            // Sanity guard: if the row would render absurdly narrow
            // (less than 20px per unit), fall back to default sizing.
            // Avoids a buggy polygon producing unusable keys.
            if unit_w_px >= 20.0 {
                // Final overflow check: make sure the computed
                // (leading_pad + key_widths + spacing + borders)
                // doesn't exceed the polygon's RIGHT edge (not the
                // screen edge — for left-anchored polygons those
                // differ). Past bug: this checked surface_w, which
                // let right-anchored rows overflow past the screen
                // edge when row_right_px happened to be surface_w
                // (always true for right-anchored) but the actual
                // render was slightly larger due to f64 rounding +
                // CSS padding under-counted.
                //
                // Use a small safety margin (8px) to absorb any
                // residual CSS chrome we didn't account for (key
                // padding, the row Box's own margins).
                const SAFETY_MARGIN_PX: f64 = 16.0;
                let max_render_right = row_right_px - SAFETY_MARGIN_PX;
                let total_render =
                    row_left_px + unit_w_px * total_units + spacing_total + border_total;
                if total_render > max_render_right {
                    let overshoot = total_render - max_render_right;
                    let unit_w_adjusted =
                        ((usable_width - SAFETY_MARGIN_PX - overshoot)
                            .max(20.0 * total_units))
                            / total_units;
                    tracing::warn!(
                        "thumb-arc: row y={:.2} would overflow polygon right edge \
                         by {:.1}px; shrinking unit_w from {:.1} to {:.1}",
                        row_y_norm,
                        overshoot,
                        unit_w_px,
                        unit_w_adjusted
                    );
                    return RowGeometry {
                        leading_pad_px: row_left_px,
                        unit_w_px: unit_w_adjusted,
                    };
                }
                return RowGeometry {
                    leading_pad_px: row_left_px,
                    unit_w_px,
                };
            } else {
                tracing::warn!(
                    "thumb-arc: row at y={:.2} produced unit_w={:.1}px; \
                     too narrow, falling back to default geometry",
                    row_y_norm,
                    unit_w_px
                );
            }
        }
        // Polygon didn't intersect this y at all (e.g. arc doesn't reach
        // the top row of the keyboard). Fall through to default sizing
        // — the user will need to recalibrate. Warn so we have a signal.
        tracing::debug!(
            "thumb-arc: polygon doesn't cover y={:.2}; using default geometry",
            row_y_norm
        );
    }

    // No compaction → standard full-width behavior.
    let leading_pad_px = if !layout.ortholinear && row.offset > 0.0 {
        row.offset as f64 * default_unit_w
    } else {
        0.0
    };

    RowGeometry {
        leading_pad_px,
        unit_w_px: default_unit_w,
    }
}

/// Compute the [x_min, x_max] range where a polygon intersects the
/// horizontal line at `y_target`. All coordinates are in normalized
/// [0,1] space. Returns None if the polygon doesn't cover that y at all.
///
/// Algorithm: a scanline pass. For each polygon edge, check if it
/// straddles `y_target`. If so, interpolate to find the x where the edge
/// crosses. The min/max of all such x's is the answer.
///
/// Note: this assumes the polygon is convex enough that there's a single
/// connected x-range at every covered y. For non-convex polygons (e.g. a
/// C-shape) you'd get an inset region; we use convex-only polygons in
/// thumb-arc defaults so this is fine.
/// Per-row crunch factor for single-hand mode. The polygon already
/// shapes the keyboard into a thumb-friendly wedge, but the middle
/// and bottom letter rows still feel "too spread out" to many users
/// — the thumb has to travel further laterally per tap than is
/// comfortable.
///
/// This function returns an additional shrink factor (≤ 1.0) to
/// apply to the polygon's width at the row's vertical center:
///
///   y_norm ranges:
///   - top row (< 0.25):           1.00 — already narrow at tip
///   - middle letter row (< 0.50): 0.82 — peak crunch
///   - bottom letter row (< 0.75): 0.88 — moderate crunch
///   - spacebar row (≥ 0.75):      0.98 — minimal (space needs width
///                                        for arrow-swipe + canvas)
///
/// Implementation chose constants empirically — the goal is "noticeably
/// tighter without breaking the layout". Past 20% additional crunch the
/// middle row keys get too narrow on small phones; under 10% the user
/// barely notices.
pub(crate) fn middle_row_crunch_factor(y_norm: f64) -> f64 {
    if y_norm < 0.25 {
        1.00
    } else if y_norm < 0.50 {
        0.82
    } else if y_norm < 0.75 {
        0.88
    } else {
        0.98
    }
}

/// Shrink an x-range `[x_min, x_max]` by `crunch` (in [0, 1])
/// keeping the anchor edge fixed. For a right-anchored polygon the
/// RIGHT edge stays put and the LEFT edge moves rightward; for a
/// left-anchored polygon the opposite. crunch=1.0 is a no-op.
///
/// Returns the new `(x_min, x_max)`.
pub(crate) fn apply_crunch_anchored(
    x_min: f64,
    x_max: f64,
    crunch: f64,
    anchor: crate::config::ThumbAnchor,
) -> (f64, f64) {
    // Guard: nonsensical or no-op inputs pass through.
    if !crunch.is_finite() || crunch >= 1.0 || crunch <= 0.0 {
        return (x_min, x_max);
    }
    if x_max <= x_min {
        return (x_min, x_max);
    }
    let width = x_max - x_min;
    let new_width = width * crunch;
    match anchor {
        crate::config::ThumbAnchor::BottomRight => {
            // Right edge fixed; left edge moves rightward.
            let new_x_min = x_max - new_width;
            (new_x_min, x_max)
        }
        crate::config::ThumbAnchor::BottomLeft => {
            // Left edge fixed; right edge moves leftward.
            let new_x_max = x_min + new_width;
            (x_min, new_x_max)
        }
    }
}

fn polygon_x_range(polygon: &[(f64, f64)], y_target: f64) -> Option<(f64, f64)> {
    if polygon.len() < 3 {
        return None;
    }

    let mut x_min = f64::MAX;
    let mut x_max = f64::MIN;
    let n = polygon.len();

    for i in 0..n {
        let (x0, y0) = polygon[i];
        let (x1, y1) = polygon[(i + 1) % n];

        // Skip horizontal edges — they don't "cross" a horizontal line
        // in a useful way (either at it or not, but no interpolation
        // needed since the endpoints already give us min/max x).
        if (y0 - y1).abs() < 1e-9 {
            // If we're sitting on the line, include both endpoints.
            if (y0 - y_target).abs() < 1e-9 {
                x_min = x_min.min(x0).min(x1);
                x_max = x_max.max(x0).max(x1);
            }
            continue;
        }

        // Check if y_target is between y0 and y1 (inclusive of one end
        // to avoid double-counting at vertices). Use [y0, y1) or [y1, y0)
        // depending on direction.
        let (y_lo, y_hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        if y_target < y_lo || y_target > y_hi {
            continue;
        }

        // Linear interpolation: find x where this edge crosses y_target.
        let t = (y_target - y0) / (y1 - y0);
        let x_at_y = x0 + t * (x1 - x0);
        x_min = x_min.min(x_at_y);
        x_max = x_max.max(x_at_y);
    }

    if x_min > x_max {
        None
    } else {
        Some((x_min, x_max))
    }
}

/// Look up the primary monitor's logical width in pixels. Returns
/// None if we can't reach a display or monitor.
fn monitor_width_logical() -> Option<f64> {
    use gtk4::prelude::*;
    let display = gtk4::gdk::Display::default()?;
    let monitors = display.monitors();
    let monitor = monitors.item(0)?;
    let monitor = monitor.downcast::<gtk4::gdk::Monitor>().ok()?;
    Some(monitor.geometry().width() as f64)
}

/// New row builder that takes a `RowGeometry` (so per-row sizing is
/// possible). When `geom.leading_pad_px` differs from what the legacy
/// stagger would compute, we override; otherwise we delegate to the
/// legacy path. Used for thumb-arc-compacted rendering.
fn build_row_static_v2(
    row: &crate::layouts::Row,
    state: KeypadState,
    layout: &Layout,
    engine: &KeyboardEngine,
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    geom: &RowGeometry,
) -> gtk4::Box {
    let unit_w = geom.unit_w_px as i32;
    let leading_pad_px = geom.leading_pad_px as i32;

    // The legacy stagger pad (what build_row_static would compute on
    // its own). If our arc-fitted pad equals this, no override needed.
    let legacy_stagger_pad = if !layout.ortholinear && row.offset > 0.0 {
        (row.offset * unit_w as f32) as i32
    } else {
        0
    };

    let pad_override = if (leading_pad_px - legacy_stagger_pad).abs() > 2 {
        Some(leading_pad_px)
    } else {
        None
    };

    build_row_static_with_pad(row, state, layout, engine, widgets, unit_w, pad_override)
}

/// Compute the per-unit-column width in logical pixels, based on the
/// current screen / surface size and the layout's row geometry.
///
/// Bug history: previously this routine only budgeted `cols * unit_w +
/// spacing`. That under-counted three things:
///   1. The keyboard_box's outer margins (8px start + 8px end)
///   2. Per-row leading pad for staggered rows (up to `max_offset *
///      unit_w` of horizontal space, e.g. 0.5 × unit_w for QWERTY row 3)
///   3. Per-key CSS borders (1px × 2 = 2px per key on the rendered
///      surface that we never accounted for)
///
/// Result: staggered rows overflowed the right edge by ~0.5 unit
/// (≈30-40px), and even ortholinear was nudged slightly over due to
/// borders + margins. Compact mode "fit" only because its tighter
/// padding/margin CSS happened to absorb the over-budget.
///
/// Fix: subtract the box margins AND budget for the worst-case row
/// offset by computing on `effective_cols = cols + max_row_offset`.
fn compute_unit_width(
    reference_widget: &gtk4::Box,
    grid_columns: f32,
    rows: &[crate::layouts::Row],
    ortholinear: bool,
) -> i32 {
    use gtk4::prelude::WidgetExt;

    // Prefer the widget's allocated width if it has one (after first
    // size-allocate). When the widget is already laid out, its
    // `width()` reports the outer-content area WITH margins ALREADY
    // EXCLUDED — so we must not subtract OUTER_MARGIN again from it.
    //
    // The fallback path reads the monitor geometry, which is the
    // FULL screen width including the keyboard_box's margins, so
    // we DO subtract OUTER_MARGIN there.
    //
    // We encode the difference by passing a flag (margins_already_
    // applied) to the pure helper.
    let alloc_w = reference_widget.width();
    if alloc_w > 32 {
        return compute_unit_width_from_screen_w_scaled_v2(
            alloc_w,
            grid_columns,
            rows,
            ortholinear,
            WIDTH_SCALE.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0,
            /* margins_already_applied */ true,
        );
    }
    let screen_w_logical = if let Some(display) = gtk4::gdk::Display::default() {
        if let Some(monitor) = display.monitors().item(0) {
            if let Ok(m) = monitor.downcast::<gtk4::gdk::Monitor>() {
                let geom = m.geometry();
                geom.width()
            } else {
                0
            }
        } else {
            0
        }
    } else {
        0
    };

    compute_unit_width_from_screen_w(screen_w_logical, grid_columns, rows, ortholinear)
}

/// Pure version of [`compute_unit_width`] that takes the screen width
/// directly. Extracted so the math is testable without a live GTK
/// context — the test binary (and `#[test]` cases) can call this with
/// synthetic screen widths to validate row overflow against any of
/// our layout combinations.
///
/// All padding/border/spacing accounting is identical to the GTK-driven
/// caller; the only difference is `screen_w_logical` arrives as a
/// parameter instead of being read from a widget.
fn compute_unit_width_from_screen_w(
    screen_w_logical: i32,
    grid_columns: f32,
    rows: &[crate::layouts::Row],
    ortholinear: bool,
) -> i32 {
    compute_unit_width_from_screen_w_scaled(
        screen_w_logical,
        grid_columns,
        rows,
        ortholinear,
        WIDTH_SCALE.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0,
    )
}

/// Like compute_unit_width_from_screen_w but applies a final
/// width-scale multiplier (e.g. 1.03 → keys 3% bigger). The caller
/// is expected to pre-clamp the scale to a sensible range; passing
/// 0.0 or NaN will produce zero-width keys, which we guard against
/// with a min-floor.
fn compute_unit_width_from_screen_w_scaled(
    screen_w_logical: i32,
    grid_columns: f32,
    rows: &[crate::layouts::Row],
    ortholinear: bool,
    width_scale: f32,
) -> i32 {
    compute_unit_width_from_screen_w_scaled_v2(
        screen_w_logical,
        grid_columns,
        rows,
        ortholinear,
        width_scale,
        /* margins_already_applied */ false,
    )
}

/// Inner helper. When `margins_already_applied` is true, the
/// caller's `screen_w_logical` value already has the keyboard_box's
/// outer margins subtracted (because GTK's `Widget::width()`
/// reports the allocated width excluding margins). Skipping the
/// double-subtract was the source of a long-standing
/// rightmost-column-overflow bug.
fn compute_unit_width_from_screen_w_scaled_v2(
    screen_w_logical: i32,
    grid_columns: f32,
    rows: &[crate::layouts::Row],
    ortholinear: bool,
    width_scale: f32,
    margins_already_applied: bool,
) -> i32 {
    let scale = if width_scale.is_finite() && width_scale > 0.0 {
        width_scale.clamp(0.5, 2.0)
    } else {
        1.0
    };
    if screen_w_logical < 32 {
        return 60;
    }

    // Cap the effective width. On a phone the keyboard spans the
    // full (narrow) screen, which is correct. On a laptop / wide
    // monitor the layer-shell Left+Right anchor makes the surface
    // as wide as the display (e.g. 1920px), which would size each
    // unit-column at ~190px — absurdly large keys that also tend to
    // overflow the rightmost column. A touch keyboard wider than
    // ~720px is ergonomically wrong regardless (thumbs can't reach
    // across), so we clamp here. The keyboard_box itself is
    // center-aligned and width-capped to match (see KeyboardView::
    // new) so the keys render centered rather than stretched.
    //
    // SCALE-AWARE CAP: width_scale lets the user explicitly request
    // bigger keys (up to 2x). If we ignored scale here, the cap math
    // would compute unit_w against 720px, then SCALE THAT, producing
    // keys that overflow the still-720px keyboard_box (severe right-
    // edge clipping at 2x). Scale the cap with the user's request so
    // unit_w + scale together stay inside the rendered surface.
    // Absolute ceiling 1100px so even 2x on a 4K display doesn't
    // sprawl unbounded.
    let scaled_cap = ((MAX_KEYBOARD_WIDTH as f32 * scale).round() as i32).min(1100);
    let screen_w_logical = screen_w_logical.min(scaled_cap);

    // Outer margin set in KeyboardView::new (margin_start=8 + end=8).
    // Only subtract if the caller hasn't already excluded margins.
    const OUTER_MARGIN: i32 = 8 * 2;
    // .keyboard-view CSS has padding:4px which is INSIDE the
    // allocated box. Always applies — never excluded by the caller.
    const KB_VIEW_PADDING: i32 = 4 * 2;
    // .key-button has border:1px + padding:2px on each side = 6px
    // of horizontal chrome per key (not part of the rendered body).
    const BORDER_PER_KEY: f32 = 6.0;

    let max_offset: f32 = if ortholinear {
        0.0
    } else {
        rows.iter().map(|r| r.offset).fold(0.0_f32, f32::max)
    };

    let cols = grid_columns.max(1.0);
    let effective_cols = cols + max_offset;
    let spacing_total = (4.0 * (cols - 1.0)) as i32;
    let border_total = (BORDER_PER_KEY * cols) as i32;
    let safety_margin = 8;

    let outer = if margins_already_applied {
        0
    } else {
        OUTER_MARGIN
    };
    let usable = (screen_w_logical
        - outer
        - KB_VIEW_PADDING
        - spacing_total
        - border_total
        - safety_margin)
        .max(50);
    let mut unit_w = (usable as f32 / effective_cols).floor() as i32;

    if cols < 6.0 {
        let cap = 55;
        if unit_w > cap {
            unit_w = cap;
        }
    }

    let scaled = (unit_w as f32 * scale).floor() as i32;
    scaled.max(20)
}

/// Feed a steno gesture into the steno engine. If the gesture pairs
/// with a pending stroke on the opposite thumb, the resolved pair-
/// stroke commits as text immediately. Otherwise the gesture goes
/// pending and a glib timeout is scheduled — if no pair arrives
/// within `PAIR_WINDOW_MS`, the lone stroke commits as a solo
/// (single phoneme).
///
/// Both pair and solo commits emit `EngineInput::StenoCommit(text)`
/// to commit the phonemes verbatim without auto-space (steno
/// phonemes chain directly: `k` + `a` + `t` → `kat`, not `k a t`).
/// The resolved-pair description is also posted to the microterm
/// for visibility.
fn handle_steno_gesture(
    engine: &KeyboardEngine,
    button: crate::steno::StenoButton,
    gesture: crate::steno::StenoGesture,
) {
    // StenoButton is Copy so the `button` argument can be used both
    // for the stroke and for the post-move side() lookup. Snapshot
    // side here for clarity.
    let side = button.side();
    let stroke = crate::steno::StenoStroke { button, gesture };
    let steno = crate::steno::global();
    let (commit_opt, gen) = steno.feed_gesture(stroke);

    // If the stroke paired immediately, commit and return.
    if let Some(commit) = commit_opt {
        info!("steno: {} (paired)", commit.description);
        engine.handle_input(EngineInput::StenoCommit(commit.text.clone()));
        surface_steno_commit(&commit);
        return;
    }

    // Otherwise: pending. Schedule a timeout. When it fires, ask
    // the engine for a solo flush — engine returns None if the
    // stroke was already committed via pairing (stale closure).
    let engine_clone = engine.clone();
    glib::timeout_add_local(
        std::time::Duration::from_millis(crate::steno::PAIR_WINDOW_MS + 10),
        move || {
            let steno = crate::steno::global();
            if let Some(commit) = steno.flush_solo(side, gen) {
                info!("steno: {} (solo)", commit.description);
                engine_clone.handle_input(EngineInput::StenoCommit(commit.text.clone()));
                surface_steno_commit(&commit);
            }
            glib::ControlFlow::Break
        },
    );
}

/// Surface a steno commit to the microterm. Briefs (whole words)
/// just toast their description. Phoneme commits with candidates
/// post the TAPPABLE suggestion ribbon (buffer hint + candidate
/// chips); tapping a chip calls accept_word via the microterm
/// bridge. Phoneme commits with no candidates fall back to a toast
/// showing the raw phoneme run.
fn surface_steno_commit(commit: &crate::steno::StenoCommit) {
    if commit.was_brief {
        crate::ui::window::post_microterm_status(commit.description.clone());
        return;
    }
    if commit.candidates.is_empty() {
        // No dictionary match yet — show the phoneme buffer so the
        // user can see what they've accumulated.
        let buf = crate::steno::global().buffer_snapshot().join(" ");
        crate::ui::window::post_microterm_status(format!("[{}]", buf));
        return;
    }
    let buffer = crate::steno::global().buffer_snapshot().join(" ");
    let candidates: Vec<(String, bool)> = commit
        .candidates
        .iter()
        .map(|c| (c.word.clone(), !c.exact))
        .collect();
    crate::ui::window::post_steno_suggestions(buffer, candidates);
}

/// Should this key action repeat while held? Hardware keyboards
/// repeat almost everything (every letter, digit, punctuation,
/// arrows, backspace). Touch keyboards conventionally limit repeat
/// to a few keys that benefit:
///   - Backspace: very useful for fast deletion
///   - Arrow keys: useful for cursor navigation
///   - Tab: very rarely held but consistent
///   - Space: NO — holding space is a common springboard gesture
/// Letters and digits are NOT repeatable because holding to type
/// "aaaaaaaa" is rare in touch UX and would conflict with long-
/// press popovers (accents, alternates).
fn is_key_repeatable(action: &KeyAction) -> bool {
    match action {
        KeyAction::Backspace | KeyAction::Tab => true,
        KeyAction::Keysym(name) => {
            matches!(
                name.as_str(),
                "Up" | "Down" | "Left" | "Right" | "BackSpace" | "Tab"
            )
        }
        _ => false,
    }
}

/// Free-function version of `KeyboardView::build_row` so the engine
/// listener closure can call it without holding a `&self`.
fn build_row_static(
    row: &Row,
    state: KeypadState,
    layout: &Layout,
    engine: &KeyboardEngine,
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    unit_w: i32,
) -> gtk4::Box {
    build_row_static_with_pad(row, state, layout, engine, widgets, unit_w, None)
}

/// Run recognition on the current stroke buffer and commit if
/// confidence is high; otherwise post a low-confidence hint and
/// leave the strokes in place for /train. Always closes the
/// canvas (one-shot semantics) regardless of recognition result.
/// Used by the confirm ✓ button and the spacebar long-press path.
fn finalize_canvas(engine: &KeyboardEngine) {
    let buf = crate::keyboard::character_draw::current_strokes();
    if buf.is_empty() {
        crate::keyboard::character_draw::close_canvas();
        crate::ui::window::post_microterm_status("Canvas closed (empty)");
        return;
    }
    match crate::keyboard::character_draw::recognize_candidates(&buf, 3).as_slice() {
        [(recognized, score), ..] if *score >= 0.5 => {
            tracing::info!(
                "canvas: recognized {} (score={:.2})",
                recognized,
                score
            );
            // StenoCommit, not SwipeWord — recognized characters
            // are single chars, so auto-space's word-chain semantics
            // would wrongly prepend spaces between consecutive
            // canvas commits or between a swipe-typed word and a
            // canvas commit. See the cardinal-swipe dispatch for
            // the same reasoning.
            engine.handle_input(EngineInput::StenoCommit(recognized.to_string()));
            crate::ui::window::post_microterm_status(format!(
                "✓ {} ({:.0}%)",
                recognized,
                score * 100.0
            ));
            crate::keyboard::character_draw::clear_strokes();
            crate::keyboard::character_draw::close_canvas();
        }
        candidates if !candidates.is_empty() => {
            let summary: String = candidates
                .iter()
                .map(|(d, s)| format!("{} {:.0}%", d, s * 100.0))
                .collect::<Vec<_>>()
                .join("  ·  ");
            tracing::info!("canvas: low confidence; candidates: {}", summary);
            crate::ui::window::post_microterm_status(format!(
                "Low conf · {} · /train to teach",
                summary
            ));
            // Leave canvas open and strokes intact — user can
            // retry or /train.
        }
        _ => {
            crate::ui::window::post_microterm_status(
                "No match — /train <char> to teach this drawing",
            );
            // Leave open for retry.
        }
    }
}

/// Variant of `build_row_static` that allows an explicit leading-pad
/// override (in pixels). Used by `build_row_static_v2` when the
/// thumb-arc fitting wants a non-standard leading pad. `None` falls
/// back to the legacy `row.offset * unit_w` behavior.
fn build_row_static_with_pad(
    row: &Row,
    state: KeypadState,
    layout: &Layout,
    engine: &KeyboardEngine,
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    unit_w: i32,
    leading_pad_override: Option<i32>,
) -> gtk4::Box {
    // For narrow ortho layouts (e.g. the 4-column numpad), keys
    // would otherwise be left-aligned with empty space on the
    // right. Center them instead by shrinking the row to its
    // content width and letting the parent's horizontal alignment
    // handle the gap. Detected by grid_columns < 6 (the qwerty/
    // dvorak/cardinal layouts are all ≥10).
    //
    // The wide-screen overflow fix lives at the keyboard_box level
    // (it's width-capped + centered on displays wider than
    // MAX_KEYBOARD_WIDTH), so the rows here always operate within a
    // phone-width surface regardless of the physical display.
    // Centering trigger: ortholinear layouts with few columns (the
    // stenotype layouts in particular) should be horizontally
    // CENTERED on a wide display rather than stretched. Without this,
    // 4 huge cells span an entire laptop monitor.
    //
    // EXCEPTION: the arrows tray (`name == "arrows"`) is also a
    // 4-column ortholinear layout but it's a navigation surface
    // meant to be tap-target-rich at any width. Centering it
    // creates dead zones on the left and right of the keyboard
    // surface where taps register on no key — perceived as "the
    // main zone doesn't do anything" + "the right side is finnicky".
    // Excluding arrows from narrow_centered keeps the cluster
    // stretched to the full keyboard width (still capped at
    // MAX_KEYBOARD_WIDTH on wide displays).
    let narrow_centered = layout.ortholinear
        && layout.grid_columns < 6.0
        && layout.name != "arrows";

    let row_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(4)
        .hexpand(!narrow_centered)
        .vexpand(true)
        .halign(if narrow_centered {
            gtk4::Align::Center
        } else {
            gtk4::Align::Fill
        })
        .css_classes(["keyboard-row"])
        .build();

    // Leading pad: either an explicit override (used by thumb-arc
    // fitting) or the legacy stagger pad. The override wins if present
    // and non-negative.
    let leading_pad = match leading_pad_override {
        Some(px) if px > 0 => px,
        Some(_) => 0,
        None => {
            if !layout.ortholinear && row.offset > 0.0 {
                (row.offset * unit_w as f32) as i32
            } else {
                0
            }
        }
    };
    if leading_pad > 0 {
        let pad = gtk4::Box::builder().width_request(leading_pad).build();
        row_box.append(&pad);
    }

    let mut total_units: f32 = 0.0;
    for key in &row.keys {
        total_units += key.width;

        // Hide voice / llm keys when their feature is disabled in
        // config. We don't remove the cell — that would shift the
        // row's keys and confuse spatial muscle memory. Instead we
        // substitute a NoOp key (visible but inert) at the same
        // size + position.
        let effective_key = match key.action {
            KeyAction::Voice if !crate::ui::window::voice_key_visible() => {
                Key::special("", KeyAction::NoOp).with_width(key.width)
            }
            KeyAction::Llm if !crate::ui::window::llm_key_visible() => {
                Key::special("", KeyAction::NoOp).with_width(key.width)
            }
            _ => key.clone(),
        };

        let widget = KeyWidget::new(effective_key.clone());
        widget.update_for_state(state);

        widget
            .widget()
            .set_width_request((unit_w as f32 * effective_key.width) as i32);

        let engine_clone = engine.clone();
        // Capture this key's own button handle so the tap callback can
        // animate it directly (O(1)) rather than scanning the whole
        // widget list by label+action (O(n) string comparisons) on
        // every keystroke. `gtk4::Button` clone is a refcount bump.
        let own_button = widget.widget().clone();
        widget.add_pressed_callback(move |k| {
            // Synchronous tap policy:
            //
            //   - If SWIPE_GESTURE_ACTIVE is set, a drag has accumulated
            //     significant movement in this gesture cycle. The click
            //     event that just arrived is the release-end of that
            //     drag (release happened to land inside button bounds,
            //     which is how Button.clicked fires). Drop the tap;
            //     the drag dispatcher will handle the gesture.
            //
            //   - Otherwise, fire the tap immediately. No timer, no
            //     queue eviction. We trust that "no significant
            //     movement happened" is sufficient evidence that this
            //     was a real tap.
            //
            // The flag is set ONLY in drag_update when cumulative
            // movement crosses a threshold — NOT in drag_begin. This
            // means a tap with sub-threshold finger jitter (which can
            // still trigger drag_begin in GTK) still commits, because
            // the flag never got set.
            if SWIPE_GESTURE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
                debug!(
                    "Tap dropped (swipe in this gesture): {} ({:?})",
                    k.label, k.action
                );
                return;
            }
            // When the drawing canvas is open, taps anywhere on
            // the keyboard are meant as stroke starts (well, drag
            // starts — but a single tap is a degenerate stroke of
            // one point and effectively a noop). Suppress the key
            // commit so the canvas isn't accidentally typing
            // letters while the user is drawing.
            if crate::keyboard::character_draw::canvas_open() {
                debug!("Tap dropped (canvas open): {} ({:?})", k.label, k.action);
                return;
            }
            // Steno key tap → feed the steno engine, not the
            // standard text commit path. Tap means
            // StenoGesture::Tap; cardinal swipes are routed
            // separately by the swipe dispatcher.
            if let KeyAction::Steno(button_id) = &k.action {
                if let Some(button) = crate::steno::StenoButton::parse(button_id) {
                    handle_steno_gesture(
                        &engine_clone,
                        button,
                        crate::steno::StenoGesture::Tap,
                    );
                    return;
                }
            }
            debug!("Tap: {} ({:?})", k.label, k.action);
            engine_clone.handle_input(EngineInput::KeyTap(k.clone()));
            // Animate this key's own widget directly — no list scan.
            crate::ui::animations::Animator::key_press_pulse(&own_button);
        });

        // Key-repeat: for keys that benefit from a hardware-keyboard-
        // like repeat-while-held (Backspace, arrow keys), attach a
        // press/release pair that drives a repeat timer. Standard
        // letter keys do NOT repeat — that would be surprising
        // (typing "a" by holding shouldn't produce "aaaaaaaa").
        //
        // The repeat logic: on press, start a timer that fires after
        // KEY_REPEAT.delay_ms with the first repeat, then continues
        // every KEY_REPEAT.interval_ms until release. The initial
        // tap is fired by the regular click handler above (on release
        // or after the swipe detector resolves). The repeat-fires are
        // ADDITIONAL emissions while the finger is held.
        //
        // The Arc is HOISTED out of the `if is_key_repeatable` block
        // so the Backspace long-press handler (synonym trigger) can
        // also cancel the timer. Without that sharing, a long-press
        // for synonym arrives just after the first key-repeat fires
        // — which has already eaten a char off the last commit, so
        // Synonym ends up operating on a partial word.
        let key_repeat_arc: Rc<RefCell<Option<glib::SourceId>>> =
            Rc::new(RefCell::new(None));
        if is_key_repeatable(&effective_key.action) {
            let engine_for_repeat = engine.clone();
            let key_repeat_arc_for_press = key_repeat_arc.clone();
            let key_repeat_arc_for_release = key_repeat_arc.clone();
            widget.add_press_release_callbacks(
                move |k| {
                    // Cancel any stale prior timer (defensive — e.g.
                    // if a release somehow didn't fire for a previous
                    // press).
                    if let Some(id) = key_repeat_arc_for_press.borrow_mut().take() {
                        id.remove();
                    }
                    // Snapshot config so disabled/enabled and timing
                    // changes apply on the NEXT press. Apply the
                    // per-action override (arrow keys get a faster
                    // delay + interval for cursor navigation).
                    let cfg = KEY_REPEAT_CONFIG.lock().for_action(&k.action);
                    if !cfg.enabled {
                        return;
                    }
                    // Schedule the FIRST repeat after delay_ms. The
                    // initial press itself emits via the regular
                    // click handler; the repeat timer fires
                    // additional events at interval_ms cadence.
                    let key_for_timer = k.clone();
                    let engine_inner = engine_for_repeat.clone();
                    let key_repeat_inner = key_repeat_arc_for_press.clone();
                    let delay = std::time::Duration::from_millis(cfg.delay_ms as u64);
                    let interval =
                        std::time::Duration::from_millis(cfg.interval_ms as u64);
                    let id = glib::timeout_add_local(delay, move || {
                        // First repeat fires here. Emit the key and
                        // re-schedule at interval_ms.
                        engine_inner.handle_input(EngineInput::KeyTap(
                            key_for_timer.clone(),
                        ));
                        // Replace this delay-timer with an interval-
                        // timer so further repeats happen at the
                        // faster cadence.
                        let key_for_loop = key_for_timer.clone();
                        let engine_for_loop = engine_inner.clone();
                        let key_repeat_for_loop = key_repeat_inner.clone();
                        let loop_id = glib::timeout_add_local(interval, move || {
                            engine_for_loop.handle_input(EngineInput::KeyTap(
                                key_for_loop.clone(),
                            ));
                            glib::ControlFlow::Continue
                        });
                        *key_repeat_for_loop.borrow_mut() = Some(loop_id);
                        // The original delay timer is replaced; tell
                        // glib we're done with it.
                        glib::ControlFlow::Break
                    });
                    *key_repeat_arc_for_press.borrow_mut() = Some(id);
                },
                move |_k| {
                    // Release — cancel any pending or running
                    // repeat timer for this key.
                    if let Some(id) = key_repeat_arc_for_release.borrow_mut().take() {
                        id.remove();
                    }
                },
            );
        }

        // Long-press handler — currently only meaningful on the
        // spacebar (where it toggles the microterm status display
        // via the bridge). Other keys could use this in the future
        // for popovers or accents; for now we gate by KeyAction so
        // we don't spawn unused gesture controllers on every letter.
        //
        // We also gate on SWIPE_GESTURE_ACTIVE inside the handler:
        // if the user has already moved their finger significantly,
        // this is a swipe (springboard / swipe-typing), not a
        // long-press. GestureLongPress fires after ~500ms regardless
        // of movement; without this check, a slow swipe-from-space
        // would both fire the status display AND the swipe word.
        if matches!(effective_key.action, KeyAction::Space) {
            let long_press = gtk4::GestureLongPress::new();
            let engine_for_long = engine.clone();
            let key_for_long = effective_key.clone();
            long_press.connect_pressed(move |_g, _x, _y| {
                if SWIPE_GESTURE_ACTIVE
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    debug!("Long-press on space suppressed (swipe in progress)");
                    return;
                }
                let canvas_open = crate::keyboard::character_draw::canvas_open();
                let draw_mode_on = crate::keyboard::character_draw::draw_mode_enabled();
                if canvas_open {
                    // Canvas one-shot mode: finalize via the shared
                    // helper (same flow as the ✓ button).
                    finalize_canvas(&engine_for_long);
                    return;
                }
                if draw_mode_on {
                    let buf = crate::keyboard::character_draw::current_strokes();
                    if buf.is_empty() {
                        debug!("Long-press on space: draw mode on but buffer empty — toggling off");
                        crate::keyboard::character_draw::set_draw_mode(false);
                        crate::ui::window::post_microterm_status(
                            "Character drawing: off",
                        );
                        return;
                    }
                    match crate::keyboard::character_draw::recognize_candidates(&buf, 3).as_slice() {
                        [(recognized, score), ..] if *score >= 0.5 => {
                            info!(
                                "character-draw: recognized {} (score={:.2})",
                                recognized, score
                            );
                            // StenoCommit (verbatim, no auto-space)
                            // to match finalize_canvas, which is the
                            // other recognition-commit entry point.
                            // Keeping both on the same engine
                            // primitive prevents the SwipeWord auto-
                            // space class of regression (drawing 'a'
                            // after swipe-typing 'hello' would
                            // otherwise become 'hello a').
                            engine_for_long.handle_input(
                                EngineInput::StenoCommit(recognized.to_string()),
                            );
                            crate::ui::window::post_microterm_status(
                                format!(
                                    "✓ {} ({:.0}%)",
                                    recognized,
                                    score * 100.0
                                ),
                            );
                            crate::keyboard::character_draw::clear_strokes();
                        }
                        candidates if !candidates.is_empty() => {
                            let summary: String = candidates
                                .iter()
                                .map(|(d, s)| {
                                    format!("{} {:.0}%", d, s * 100.0)
                                })
                                .collect::<Vec<_>>()
                                .join("  ·  ");
                            info!("character-draw: low confidence; candidates: {}", summary);
                            crate::ui::window::post_microterm_status(
                                format!(
                                    "Low conf · {} · /train <char> to teach",
                                    summary
                                ),
                            );
                        }
                        _ => {
                            crate::ui::window::post_microterm_status(
                                "No match — /train <char> to teach this drawing",
                            );
                        }
                    }
                    return;
                }
                debug!("Long-press on space");
                engine_for_long
                    .handle_input(EngineInput::KeyLongPress(key_for_long.clone()));
            });
            widget.widget().add_controller(long_press);

            // Drop target: when a voice-bar word chip is dropped on
            // the spacebar, commit ONLY that word. This lets the
            // user pick a single word out of a noisy partial
            // transcript without having to wait for the final.
            let drop_target = gtk4::DropTarget::new(
                glib::types::Type::STRING,
                gtk4::gdk::DragAction::COPY,
            );
            let engine_for_drop = engine.clone();
            drop_target.connect_drop(move |_target, value, _x, _y| {
                let word: Option<String> = value.get().ok();
                if let Some(mut word) = word {
                    // Append a trailing space so consecutive
                    // commits don't run together.
                    if !word.ends_with(char::is_whitespace) {
                        word.push(' ');
                    }
                    debug!("Drop on space: committing word {:?}", word);
                    engine_for_drop.handle_input(
                        EngineInput::SwipeWord(word),
                    );
                    true
                } else {
                    false
                }
            });
            widget.widget().add_controller(drop_target);
        }

        // Backspace also accepts drops: dropping a word here means
        // "delete the last commit" (rather than a per-word concept
        // — we don't track per-word commits separately). It's a
        // shortcut equivalent to a tap on backspace, exposed here
        // because the voice-bar word-drag UI invites finding a
        // destination for each chip.
        if matches!(effective_key.action, KeyAction::Backspace) {
            let drop_target = gtk4::DropTarget::new(
                glib::types::Type::STRING,
                gtk4::gdk::DragAction::COPY,
            );
            let engine_for_drop = engine.clone();
            drop_target.connect_drop(move |_target, _value, _x, _y| {
                debug!("Drop on backspace: triggering undo");
                engine_for_drop.handle_input(EngineInput::Undo);
                true
            });
            widget.widget().add_controller(drop_target);

            // Long-press on backspace: find a synonym for the most
            // recently committed word. The engine looks at its
            // undo_stack, removes the prior commit, and submits an
            // LLM Synonym task. When the LLM result lands, the
            // engine commits the synonym (which becomes the new
            // top of the undo stack).
            let long_press = gtk4::GestureLongPress::new();
            let engine_for_long = engine.clone();
            let key_repeat_arc_for_long = key_repeat_arc.clone();
            long_press.connect_pressed(move |_g, _x, _y| {
                if SWIPE_GESTURE_ACTIVE
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    return;
                }
                // CRITICAL: cancel the running key-repeat before we
                // peek_last_commit. The repeat timer fires at
                // delay_ms (default 400ms) — long-press fires at
                // GestureLongPress's default ~500ms — so by the
                // time we get here, the first backspace repeat has
                // already eaten a char off the last commit. Without
                // cancelling the timer, MORE backspaces fire while
                // the LLM round-trip is in flight (1-10s), all
                // operating against an undo stack the user is no
                // longer in control of. Cancel here stops further
                // damage; the one or two repeats that already fired
                // mean Synonym sees a slightly-shrunk word, but the
                // user can recover by tapping Undo.
                if let Some(id) = key_repeat_arc_for_long.borrow_mut().take() {
                    id.remove();
                }
                debug!("Long-press on backspace → Synonym request");
                if let Some(last) = engine_for_long.peek_last_commit() {
                    let trimmed = last.trim().to_string();
                    if trimmed.is_empty() {
                        return;
                    }
                    // Undo first so the synonym replaces the
                    // original — otherwise we'd have both. Stash
                    // the original word so the LLM bridge can
                    // restore it on failure. If an earlier
                    // in-flight LLM request had restore data, we'd
                    // be overwriting it — restore that now so its
                    // original text isn't permanently lost.
                    if let Some(stale) =
                        crate::ui::window::set_llm_restore_text(&last)
                    {
                        tracing::info!(
                            "LLM: overlapping long-press request — \
                             restoring {} chars from earlier request",
                            stale.chars().count()
                        );
                        engine_for_long.handle_input(
                            EngineInput::LlmCommit(stale),
                        );
                    }
                    engine_for_long.handle_input(EngineInput::Undo);
                    engine_for_long.handle_input(
                        EngineInput::LlmRequest(
                            crate::llm::LlmTask::Synonym {
                                word: trimmed,
                                context: None,
                            },
                        ),
                    );
                }
            });
            widget.widget().add_controller(long_press);
        }

        // LLM key: long-press triggers a voice-dictated prompt. The
        // LLM bridge flips a "next voice transcript is for LLM"
        // flag; the user is asked (via microterm) to speak the
        // prompt. When the voice final lands, the bridge intercepts
        // it and submits as an LlmRequest rather than committing.
        if matches!(effective_key.action, KeyAction::Llm) {
            let long_press = gtk4::GestureLongPress::new();
            let engine_for_long = engine.clone();
            let key_for_long = effective_key.clone();
            long_press.connect_pressed(move |_g, _x, _y| {
                if SWIPE_GESTURE_ACTIVE
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    return;
                }
                debug!("Long-press on LLM key");
                engine_for_long
                    .handle_input(EngineInput::KeyLongPress(key_for_long.clone()));
            });
            widget.widget().add_controller(long_press);
        }

        row_box.append(widget.widget());
        widgets.borrow_mut().push(widget);
    }

    if layout.ortholinear && total_units < layout.grid_columns {
        let trailing_pad = ((layout.grid_columns - total_units) * 60.0) as i32;
        if trailing_pad > 0 {
            let pad = gtk4::Box::builder().width_request(trailing_pad).build();
            row_box.append(&pad);
        }
    }

    row_box
}

/// Recompute the on-screen position of each key widget so swipe decoding
/// can hit-test against the current layout.
///
/// We use `compute_bounds(target)` (gtk4 ≥ 4.12) which returns a
/// `graphene::Rect` describing the widget's bounds in `target`'s coordinate
/// space — exactly what we need. It replaces the deprecated pair of
/// `allocation()` + `translate_coordinates()`.
/// Build hitboxes from the current key widgets, in keyboard_box's
/// local coordinate frame. Called by drag_begin (in cardinal mode,
/// to identify what cell the start point fell on) and drag_end
/// (always — to give the gesture taxonomy current cell positions).
///
/// Performance note: GTK's `compute_bounds` is non-trivial. For a 32-
/// key keyboard with no relevant change between calls, refreshing
/// every drag-end (after refreshing at drag-begin) does ~32 redundant
/// compute_bounds. The cost is acceptable today (drags are rare,
/// per-key bounds are fast at phone-keyboard sizes); if profiling
/// shows it hot, a staleness check based on widget-count or a
/// rebuild-generation counter would let us skip the redundant
/// recompute.
/// Coordinate-frame recovery for swipe paths captured with a possibly-
/// bogus offset.
///
/// drag_begin captures `offset = kb_box.compute_bounds(overlay).map(...)`.
/// On Plasma Mobile, that call can transiently return None during
/// layer-surface map-but-not-sized windows, causing offset = (0, 0).
/// Every point in the captured path then sits in OVERLAY frame instead
/// of KB frame, which makes:
///   - decode_swipe_path's nearest-key lookup find the wrong row's
///     letters → empty word → swipe-typing line shows but no commit
///   - dispatch_cardinal_swipe's start-point hit-test miss the
///     intended cell → silent drop
///
/// At drag_end, the widget tree is typically fully laid out and
/// compute_bounds succeeds. We recompute the offset and compare to the
/// stored one. If they differ, we translate every path point by the
/// delta to bring it into KB frame.
///
/// Returns the (possibly-corrected) path. The original is consumed
/// to avoid a clone when no correction is needed.
fn recover_frame(
    mut path: Vec<crate::gestures::swipe::SwipePoint>,
    stored_offset: (f64, f64),
    kb_box: &gtk4::Box,
    overlay: &gtk4::Overlay,
) -> Vec<crate::gestures::swipe::SwipePoint> {
    use gtk4::prelude::WidgetExt;
    let Some(bounds) = kb_box.compute_bounds(overlay) else {
        // compute_bounds STILL failing — kb_box not laid out, or
        // detached. Nothing useful we can do; return path as-is and
        // let downstream silently drop (better than a panic).
        tracing::warn!(
            "recover_frame: compute_bounds still None at drag_end; \
             path stays in stored frame (offset=({:.1}, {:.1}), points={})",
            stored_offset.0, stored_offset.1, path.len()
        );
        return path;
    };
    let current = (bounds.x() as f64, bounds.y() as f64);
    let dx = current.0 - stored_offset.0;
    let dy = current.1 - stored_offset.1;
    // 1-px tolerance for sub-pixel-rounding noise. Real drift comes
    // from the (0, 0) fallback case where dx/dy ≈ 8/2 (kb_box's
    // margins) or larger.
    if dx.abs() < 1.0 && dy.abs() < 1.0 {
        return path;
    }
    tracing::warn!(
        "recover_frame: frame drift detected — stored=({:.1}, {:.1}) \
         current=({:.1}, {:.1}) delta=({:.1}, {:.1}); shifting path of {} pts",
        stored_offset.0, stored_offset.1,
        current.0, current.1,
        dx, dy, path.len()
    );
    for p in &mut path {
        p.x -= dx;
        p.y -= dy;
    }
    path
}

/// Bumped on every `rebuild_view` (widgets recreated) so the hitbox
/// cache can detect when it's stale. `refresh_hitboxes` snapshots the
/// counter after recomputing; subsequent drag_begins early-return
/// when the snapshot still matches, skipping ~30 compute_bounds GTK
/// calls + 30 Key clones per gesture. Window resize also calls
/// rebuild_view (via the size-allocate path), so resize correctness
/// follows for free.
static HITBOX_GENERATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
/// Last gen value seen at the end of refresh_hitboxes. Stored as
/// AtomicU64 so it's accessible from any closure without plumbing.
/// Initialized to u64::MAX so the first call always refreshes
/// (won't ever match a real gen of 0..).
static LAST_HITBOX_REFRESH_GEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// Force the hitbox cache to refresh on the next `drag_begin`. Used
/// as a safety net for scenarios where widget positions can change
/// without a `rebuild_view` firing — e.g. window resize on rotation,
/// monitor change, layer-shell anchor recompute. Calling this is
/// cheap (one atomic add).
pub fn invalidate_hitbox_cache() {
    HITBOX_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Read the current hitbox generation. Exposed for tests and any
/// future consumer that wants to observe cache invalidation events.
pub fn hitbox_generation() -> u64 {
    HITBOX_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

fn refresh_hitboxes(
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    container: &gtk4::Box,
    hitboxes: &Rc<RefCell<Vec<KeyHitbox>>>,
) {
    // Fast path: nothing has changed since our last refresh — the
    // current hitboxes are still valid. Skip the per-widget
    // compute_bounds GTK calls and Key clones entirely. drag_begin
    // fires this on every gesture, so the saving is meaningful even
    // for a single user session.
    let cur_gen = HITBOX_GENERATION.load(std::sync::atomic::Ordering::Relaxed);
    let last = LAST_HITBOX_REFRESH_GEN.load(std::sync::atomic::Ordering::Relaxed);
    if cur_gen == last && !hitboxes.borrow().is_empty() {
        return;
    }

    let widget_count = widgets.borrow().len();
    let mut new_boxes = Vec::with_capacity(widget_count);
    let mut compute_bounds_failures = 0usize;

    for w in widgets.borrow().iter() {
        let widget = w.widget();
        if let Some(bounds) = widget.compute_bounds(container) {
            new_boxes.push(KeyHitbox {
                key: w.key().clone(),
                x: bounds.x() as f64,
                y: bounds.y() as f64,
                width: bounds.width() as f64,
                height: bounds.height() as f64,
            });
        } else {
            // Widget exists in our Vec but compute_bounds returned
            // None — usually means it's not yet realized / laid out.
            // Count these; if there are a lot, the hitbox vec ends
            // up partial or empty, silently breaking downstream
            // hit-test and recognizer dispatch.
            compute_bounds_failures += 1;
        }
    }

    // Surface widespread compute_bounds failures. A single failure
    // could be transient (a widget being torn down mid-frame); a
    // majority failure is almost certainly the bug behind "trail
    // renders but nothing types" — the dispatch ends up with an
    // empty or near-empty boxes vec, and the recognizer has no
    // letters to map the swipe to.
    if compute_bounds_failures > 0 {
        let pct = (compute_bounds_failures * 100) / widget_count.max(1);
        if compute_bounds_failures == widget_count {
            tracing::warn!(
                "refresh_hitboxes: ALL {} widgets returned None from \
                 compute_bounds(container). Downstream swipe/cardinal \
                 dispatch will receive empty boxes vec and silently \
                 drop. Container or widget tree may not be laid out yet.",
                widget_count
            );
        } else if pct >= 25 {
            tracing::warn!(
                "refresh_hitboxes: {}/{} widgets ({}%) returned None from \
                 compute_bounds. Hitbox vec is partial; some keys won't \
                 hit-test correctly.",
                compute_bounds_failures, widget_count, pct
            );
        }
    }

    *hitboxes.borrow_mut() = new_boxes;
    LAST_HITBOX_REFRESH_GEN.store(cur_gen, std::sync::atomic::Ordering::Relaxed);
}

/// Update the recognizer's spatial map from the current hitboxes so its
/// spatial-similarity scoring uses the actual on-screen layout. Coordinates
/// are normalized to "unit-key widths" by dividing by the median key width,
/// which keeps the scale independent of window size.
/// Generation tracking for `refresh_recognizer_positions` — mirrors
/// the LAST_HITBOX_REFRESH_GEN pattern. The recognizer's spatial
/// position map is rebuilt only when the hitbox generation moved
/// since the last refresh; otherwise the existing map is reused.
/// For springboard multi-segment swipes the function is called N
/// times per gesture with identical hitboxes — all but the first
/// call previously did wasted allocation work (Vec<f64> sort +
/// HashMap of ~30 entries).
static LAST_RECOGNIZER_REFRESH_GEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

fn refresh_recognizer_positions(
    boxes: &[KeyHitbox],
    recognizer: &Rc<RefCell<GestureRecognizer>>,
) {
    if boxes.is_empty() {
        return;
    }

    // Cache check: if the hitbox generation hasn't moved since our
    // last refresh, the recognizer already holds positions derived
    // from these exact boxes. Skip the rebuild.
    let cur_gen = HITBOX_GENERATION.load(std::sync::atomic::Ordering::Relaxed);
    let last = LAST_RECOGNIZER_REFRESH_GEN.load(std::sync::atomic::Ordering::Relaxed);
    if cur_gen == last {
        return;
    }

    // Median key width to normalize coordinates.
    let mut widths: Vec<f64> = boxes
        .iter()
        .filter(|b| b.key.swipe_target)
        .map(|b| b.width)
        .collect();
    widths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let unit = widths
        .get(widths.len() / 2)
        .copied()
        .filter(|&w| w > 0.0)
        .unwrap_or(60.0);

    let mut positions = std::collections::HashMap::new();
    for b in boxes {
        if !b.key.swipe_target {
            continue;
        }
        if let Some(c) = b.key.label.chars().next() {
            let cx = (b.x + b.width / 2.0) / unit;
            let cy = (b.y + b.height / 2.0) / unit;
            // Always use lowercase for the spatial key — the recognizer
            // works in lowercase too.
            for k in c.to_lowercase() {
                positions.insert(k, (cx, cy));
            }
        }
    }
    recognizer.borrow_mut().set_key_positions(positions);
    LAST_RECOGNIZER_REFRESH_GEN.store(cur_gen, std::sync::atomic::Ordering::Relaxed);
}

/// Route a swipe path through the cardinal-grid gesture classifier.
///
/// Strategy:
///  1. The first sample of `path` tells us where the user started the
///     swipe; we hit-test that against `boxes` to find the source key.
///  2. We translate the path into coordinates relative to that key's
///     center (so e.g. a swipe-up always shows up as decreasing y in
///     the classifier's frame of reference).
///  3. Run the classifier; if it returns a non-tap gesture, look up
///     the key's `gestures` field and commit the dispatched string.
///
/// If the source key has no `gestures` map (e.g. a layout is mid-swap
/// or the user dragged off an action key onto a letter), we log and
/// silently drop the gesture rather than commit garbage.
/// If the swipe started on the space key AND its gross direction is
/// clearly cardinal (not a tiny drift), return the corresponding XKB
/// keysym name. Returns `None` for non-spacebar starts or
/// not-clearly-cardinal swipes.
///
/// Tunables:
///   - `MIN_LEN_PX`: the swipe must travel this many physical pixels to
///     count. Avoids treating a tap or microscopic drift as an arrow.
///   - `DOMINANCE`: the dominant axis must be this much larger than the
///     other (so diagonals don't accidentally fire an arrow when a true
///     up/down is intended). 1.5× is loose enough for shaky fingers.
fn detect_spacebar_arrow_swipe(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
    dx: f64,
    dy: f64,
) -> Option<&'static str> {
    let start = path.first()?;
    let source = boxes.iter().find(|b| b.contains(start.x, start.y))?;
    // Spacebar identification: any key whose action is Space.
    if !matches!(source.key.action, crate::keyboard::KeyAction::Space) {
        return None;
    }
    classify_arrow_swipe(dx, dy)
}

/// Pure direction classifier for the spacebar arrow-swipe. Separated
/// from `detect_spacebar_arrow_swipe` so the threshold logic can be
/// unit-tested without constructing a `KeyHitbox` fixture.
///
/// Returns the arrow-key name for a swipe whose total length exceeds
/// `MIN_LEN_PX` and whose dominant axis beats the other by a factor
/// of `DOMINANCE`. Diagonal-ish swipes (neither axis dominant) return
/// `None` so they don't fire an unintended arrow.
fn classify_arrow_swipe(dx: f64, dy: f64) -> Option<&'static str> {
    const MIN_LEN_PX: f64 = 28.0;
    const DOMINANCE: f64 = 1.5;

    let len = (dx * dx + dy * dy).sqrt();
    if len < MIN_LEN_PX {
        return None;
    }

    let abs_dx = dx.abs();
    let abs_dy = dy.abs();
    if abs_dx > abs_dy * DOMINANCE {
        // Horizontal swipe.
        Some(if dx > 0.0 { "Right" } else { "Left" })
    } else if abs_dy > abs_dx * DOMINANCE {
        // Vertical swipe (screen coords: dy>0 is downward).
        Some(if dy > 0.0 { "Down" } else { "Up" })
    } else {
        // Too diagonal — don't fire.
        None
    }
}

/// Bottom-pad cursor-nav swipe detector. If the user starts a swipe
/// in the bottom-padding strip (the empty space below all keys, only
/// present when cfg.layout.bottom_pad > 0), classify it as a
/// directional cursor arrow press.
///
/// Bottom_pad is where the user's thumb naturally rests on a tall
/// phone, so this is the "cursor nav without precise targeting"
/// feature. Both horizontal (Left/Right) AND vertical (Up/Down)
/// directions are supported — vertical is needed to navigate between
/// stacked editor panes, terminal history, multi-line code blocks,
/// and similar layouts.
///
/// Gating:
///   - start.y > max_key_bottom — the gesture must START in the pad
///   - dx/dy must classify as a clear cardinal (via classify_arrow_swipe)
///   - For HORIZONTAL gestures: the end must stay roughly in the pad,
///     protecting against a diagonal swipe-typing gesture that briefly
///     dips into the pad. For VERTICAL gestures the constraint is
///     geometrically impossible (a meaningful Up swipe of ≥28 px from
///     the pad necessarily ends in the keys area, by definition), so
///     it's dropped — start-in-pad is enough to disambiguate intent.
fn detect_bottom_pad_arrow(
    start: &crate::gestures::swipe::SwipePoint,
    end: &crate::gestures::swipe::SwipePoint,
    boxes: &[KeyHitbox],
    dx: f64,
    dy: f64,
) -> Option<&'static str> {
    let max_key_bottom = boxes
        .iter()
        .map(|b| b.y + b.height)
        .fold(f64::NEG_INFINITY, f64::max);
    if !max_key_bottom.is_finite() {
        return None;
    }
    let start_in_pad = start.y > max_key_bottom;
    if !start_in_pad {
        return None;
    }
    let arrow = classify_arrow_swipe(dx, dy)?;
    let is_horizontal = arrow == "Left" || arrow == "Right";
    // Horizontal: extra guard that the gesture stays roughly in the
    // pad, protecting swipe-typing from false-firing this branch.
    let end_constraint_ok = if is_horizontal {
        end.y > max_key_bottom - 10.0
    } else {
        // Vertical: end MUST go above the pad (geometrically forced),
        // so don't constrain end.y. The arrow direction already came
        // from a dominance-checked classify_arrow_swipe.
        true
    };
    if !end_constraint_ok {
        return None;
    }
    Some(arrow)
}

/// Detect a "swipe up to close keyboard" gesture. Two complementary
/// paths:
///
///   1. **Fast upward flick**: a quick gesture with high terminal
///      upward velocity, where the user "throws" the keyboard away.
///      Doesn't require the path to fully exit the keyboard — the
///      velocity itself signals dismiss intent. This is the natural,
///      casual dismiss gesture.
///
///   2. **Slow upward drag**: a deliberate drag whose terminal point
///      is well above the top edge of the keyboard's letter region
///      (> 1.5 row-heights or near the top of the screen). For users
///      who drag rather than flick.
///
/// Both paths require gross upward motion (dy < 0) and vertical
/// dominance over horizontal (so horizontal swipes don't trigger).
///
/// We deliberately split the criteria so a short up-arc on the top
/// row (which the digit gesture handles) doesn't trip flick detection
/// — top-row digit arcs are short, slow, and confined. A true dismiss
/// flick is either fast OR exits the keyboard entirely.
///
/// Returns `Some(())` if recognized. Returning a unit type rather
/// than a richer value because the caller doesn't need any data — it
/// just needs to know "this gesture was a minimize."
fn detect_minimize_fling(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
    dx: f64,
    dy: f64,
    strict: bool,
) -> Option<()> {
    use crate::gestures::boundary::LetterBoundary;

    // (1) Fast upward flick: terminal velocity drives detection.
    //
    // We compute velocity over the LAST ~60ms of the path — this is
    // what the user's finger is doing right before they lift, which
    // is the clearest signal of intent ("am I settling in for a
    // letter selection" vs "am I flicking the keyboard away"). The
    // overall dx/dy is gross displacement; terminal velocity is
    // local motion. The combination distinguishes a swipe-typing
    // trace (overall up but slowing at the end) from a true flick
    // (fast at the end).
    //
    // strict=false (qwerty/dvorak): loose thresholds; flicks work
    // easily because there are no competing cardinal-up gestures.
    // strict=true (cardinal/stenotype): higher velocity + bigger
    // displacement so cardinal cell-swipes (~80-120px, typically
    // not-super-fast) don't false-positive. A cardinal north swipe
    // at full speed across a cell rarely exceeds 1.0 px/ms over the
    // last 60ms — bumping the threshold to 2.0 px/ms keeps a clear
    // discrimination margin.
    //
    // Dominance bumped down from 1.2/1.4 to 1.0/1.2 because user
    // reports "swipe up to minimize only worked in a certain
    // region" — likely the dominance check was rejecting swipes
    // from regions where the user's hand naturally drifted
    // sideways during the upward motion. The other guards
    // (velocity AND displacement both meeting thresholds) prevent
    // a slow horizontal swipe with a tiny upward component from
    // false-triggering.
    let (flick_vel_up, flick_min_dy, flick_dominance) = if strict {
        (-2.0_f64, -120.0_f64, 1.2_f64)
    } else {
        (-1.2_f64, -50.0_f64, 1.0_f64)
    };
    let (term_vx, term_vy) = terminal_velocity(path, 60);
    if term_vy <= flick_vel_up
        && dy <= flick_min_dy
        && term_vy.abs() > term_vx.abs() * flick_dominance
    {
        info!(
            "minimize-flick: term_vy={:.2} px/ms, dy={:.0}, term_vx={:.2}, strict={}",
            term_vy, dy, term_vx, strict
        );
        return Some(());
    }

    // Backup path for clear-intent long upward swipes. If the
    // user has unambiguously swiped UP a long distance (dy <= -150
    // non-strict, -200 strict) AND the terminal velocity is upward
    // (term_vy <= -1.0 px/ms), accept the minimize regardless of
    // dominance. This catches the case where a right-handed user
    // swiping up from the right side of the keyboard naturally
    // curves the path toward screen-center (significant leftward
    // term_vx) but the gesture is clearly an upward fling.
    //
    // The unconditional dy threshold here is bigger than the
    // dominance-gated dy threshold above (-150 vs -50 non-strict)
    // specifically so this backup path requires clear intent.
    let (backup_dy_threshold, backup_vy) = if strict {
        (-200.0_f64, -1.5_f64)
    } else {
        (-150.0_f64, -1.0_f64)
    };
    if dy <= backup_dy_threshold && term_vy <= backup_vy {
        info!(
            "minimize-flick (backup path, no dominance check): term_vy={:.2}, dy={:.0}, strict={}",
            term_vy, dy, strict
        );
        return Some(());
    }

    // (3) "Exit above keyboard" — intent-based fallback that works
    // in BOTH strict and non-strict modes. Without this, cardinal-
    // grid and stenotype users couldn't dismiss the keyboard by
    // swiping up and off because the slow-drag path (4) is
    // non-strict only. The user reported "most keys not taking
    // swipe up and off keyboard as the close gesture" — cardinal
    // cells fell through both the strict flick check (which needs
    // high terminal velocity) and the slow-drag path (which is
    // gated to non-strict). Now they take the same end-above-top
    // exit check.
    //
    // Cardinal top-row cells doing a normal N cardinal swipe could
    // also exit above the keyboard (top of cell is near top of
    // keyboard, swipe goes ~80px N → ends above). We require
    // dy <= -60 to gate this against the smallest possible
    // intentional cardinal cell-cross (~50px). A 60+px upward
    // motion AND exiting the keyboard surface is clear minimize
    // intent.
    if dy <= -60.0 {
        if let Some(boundary) = LetterBoundary::from_hitboxes(boxes) {
            if let Some(end) = path.last() {
                let row_h_approx = (boundary.bottom - boundary.top) / 4.0;
                // End must be clearly above the top edge (at least
                // one row-height above). This is a stronger
                // condition than the slow-drag path's "1.5
                // row-heights" so it requires deliberate exit.
                if end.y < boundary.top - row_h_approx {
                    info!(
                        "minimize-flick (exit above keyboard, strict={}): \
                         dy={:.0}, end.y={:.0}, boundary.top={:.0}",
                        strict, dy, end.y, boundary.top
                    );
                    return Some(());
                }
            }
        }
    }

    // (2) Slow drag with path exit. Only meaningful in non-strict
    // mode — strict mode's flick path is the only way in, because a
    // slow drag in cardinal mode is almost always a real cardinal
    // gesture (or a multi-cell drag for steno chords).
    if strict {
        return None;
    }
    const SLOW_DRAG_MIN_DY: f64 = -80.0;
    const SLOW_DRAG_DOMINANCE: f64 = 1.5;
    if dy >= SLOW_DRAG_MIN_DY {
        return None;
    }
    let abs_dx = dx.abs();
    let abs_dy = dy.abs();
    if abs_dy <= abs_dx * SLOW_DRAG_DOMINANCE {
        return None;
    }

    // Confirm the path actually exits above the top edge. Without this
    // check, a swipe entirely within the letters that drifts upward
    // could be misread as a minimize.
    let boundary = LetterBoundary::from_hitboxes(boxes)?;
    let end = path.last()?;
    let row_h_approx = (boundary.bottom - boundary.top) / 4.0;
    let escape_threshold = boundary.top - row_h_approx * 1.5;
    if end.y > escape_threshold && end.y > 20.0 {
        return None;
    }
    Some(())
}

/// Compute average velocity over the trailing `window_ms` milliseconds
/// of the path. Returns (vx, vy) in px/ms. Returns (0, 0) for short
/// paths or zero-time windows.
///
/// This is "terminal" velocity in the sense that it reflects the
/// user's motion right before they lifted their finger — not the
/// instantaneous final velocity (which is noisy), and not the
/// path-average (which is dominated by the middle of long swipes).
/// 60ms is the typical sample window: long enough to smooth out
/// touch jitter, short enough that we capture only the end intent.
fn terminal_velocity(
    path: &[crate::gestures::swipe::SwipePoint],
    window_ms: i64,
) -> (f64, f64) {
    if path.len() < 2 {
        return (0.0, 0.0);
    }
    let last = path.last().unwrap();
    let cutoff = last.timestamp_ms.saturating_sub(window_ms);
    // First point in the window. If the whole path is within
    // window_ms (short, fast swipe), start_idx=0 — we'll measure
    // the entire path, which is correct: a 30ms fast flick should
    // have its full velocity captured, not zeroed out for being
    // "too short."
    let start_idx = path
        .iter()
        .position(|p| p.timestamp_ms >= cutoff)
        .unwrap_or(0);
    let start = &path[start_idx];
    let dt = (last.timestamp_ms - start.timestamp_ms).max(1) as f64;
    let vx = (last.x - start.x) / dt;
    let vy = (last.y - start.y) / dt;
    (vx, vy)
}

/// Detect a "short upward arc starting on a top-row key" that the user
/// uses for digit entry. The digit is determined by the *column* of
/// the starting key, not the letter itself — so QWERTY and Dvorak
/// both produce 1-9-0 from positions 1-10 in the top row.
///
/// Criteria:
///
///   - Swipe **starts** on a key in the top row (lowest y, ie the row
///     with `y < boundary.top + row_height` or similar). We use a
///     looser definition: starts on a key whose center's y is in the
///     top quartile of the letter region.
///   - Swipe's gross direction is upward but **constrained** — the
///     terminal point is between 8px and 1 row-height above the
///     starting key's top edge. Short, controlled.
///   - Total path length under 80px (prevents flinging).
///
/// Returns the digit character, or None.
fn detect_top_row_digit(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
) -> Option<&'static str> {
    use crate::gestures::boundary::LetterBoundary;
    const MAX_PATH_PX: f64 = 80.0;
    // Digits indexed by column 0..=9 — same for QWERTY (q-p) and
    // Dvorak ('-l). The top row of either layout always has 10 keys
    // in current designs.
    const DIGITS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];

    let start = path.first()?;
    let end = path.last()?;
    let boundary = LetterBoundary::from_hitboxes(boxes)?;

    // Total path arc length (loose proxy: end-to-end distance).
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist > MAX_PATH_PX {
        return None;
    }
    if dy >= 0.0 {
        // Not upward at all.
        return None;
    }

    // Locate the start key, confirm it's in the top row (top quartile).
    let source = boxes.iter().find(|b| b.contains(start.x, start.y))?;
    let row_h_approx = (boundary.bottom - boundary.top) / 4.0;
    if source.y > boundary.top + row_h_approx {
        // Not the top row.
        return None;
    }

    // Terminal point must be just above the key — within 1 row-height
    // of the start. Prevents this from competing with the minimize
    // fling (which terminates well above the keyboard).
    let above_amount = source.y - end.y;
    if above_amount < 8.0 || above_amount > row_h_approx {
        return None;
    }

    // Find the column index by sorting top-row keys by x. We use the
    // boxes themselves rather than the layout because hitboxes reflect
    // what's actually rendered (post-compaction, post-margin).
    let mut top_row: Vec<&KeyHitbox> = boxes
        .iter()
        .filter(|b| b.y <= boundary.top + row_h_approx)
        .collect();
    top_row.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    let col_idx = top_row
        .iter()
        .position(|b| std::ptr::eq(*b, source))?;

    DIGITS.get(col_idx).copied()
}

/// Behavior for a Normal-tap key that the user dragged from.
///
/// If the user released INSIDE the source key's bounds, this is a tap
/// (they pressed, wiggled, and released on the same key). Fire the
/// tap. If they released outside, this is a cancel — the user pressed
/// the wrong key and dragged off to abort. Don't fire anything.
///
/// This is what users expect from physical-keyboard-style buttons:
/// the touchdown highlights the key, but the commit happens at
/// release and only when the release is still on the key. Almost
/// every mobile OS implements this for soft keyboards.
fn dispatch_normal_drag(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
    engine: &KeyboardEngine,
) {
    let Some(start) = path.first() else {
        return;
    };
    let Some(end) = path.last() else {
        return;
    };

    // Find the source key from the start point.
    let source = boxes.iter().find(|b| b.contains(start.x, start.y));
    let Some(source_box) = source else {
        debug!(
            "normal drag: start ({:.1},{:.1}) didn't hit any key; ignoring",
            start.x, start.y
        );
        return;
    };

    // Check if release is still inside source bounds.
    if !source_box.contains(end.x, end.y) {
        info!(
            "normal drag: released outside source key '{}' — cancel",
            source_box.key.label
        );
        return;
    }

    // Fire as a tap.
    info!(
        "normal drag: released inside source key '{}' — tap",
        source_box.key.label
    );
    engine.handle_input(EngineInput::KeyTap(source_box.key.clone()));
}

/// Behavior for an AltKeySwipe key (iOS-style flick to alt-character).
///
/// Currently a SCAFFOLD: the AltKeySwipe behavior isn't yet enabled
/// by Key::behavior(), so this won't fire in production. When we
/// turn it on, the algorithm is:
///
///   - If the path went approximately one-cell distance in a
///     mostly-cardinal direction (N/E/S/W), pick the alt character
///     for that direction from the source key's `extended` list
///     and commit it. Otherwise, the gesture is treated as a tap
///     (user wiggled but didn't commit to a direction).
///
/// The function also serves as a place to set a per-drag flag that
/// suppresses any further recognizer fallback — once we know we
/// committed an alt-character, no swipe-type recognition should
/// also run on the same gesture.
fn dispatch_alt_key_swipe(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
    engine: &KeyboardEngine,
) {
    let Some(start) = path.first() else {
        return;
    };
    let Some(end) = path.last() else {
        return;
    };

    let source = boxes.iter().find(|b| b.contains(start.x, start.y));
    let Some(source_box) = source else {
        debug!("alt-swipe: start didn't hit any key; ignoring");
        return;
    };
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let dist = (dx * dx + dy * dy).sqrt();

    // One-cell-traversal: the released point should be roughly one
    // cell-width away from the start. We allow 0.7 to 1.5 cells of
    // distance so light overshoots still count.
    let cell_size = source_box.width.max(source_box.height);
    let min_dist = cell_size * 0.7;
    let max_dist = cell_size * 1.5;
    if dist < min_dist || dist > max_dist {
        info!(
            "alt-swipe: distance {:.1} not within one-cell range [{:.1}, {:.1}]; \
             falling back to tap",
            dist, min_dist, max_dist
        );
        engine.handle_input(EngineInput::KeyTap(source_box.key.clone()));
        return;
    }

    // Direction must be dominantly cardinal (not diagonal).
    let abs_dx = dx.abs();
    let abs_dy = dy.abs();
    const DOMINANCE: f64 = 1.5;
    let direction_idx = if abs_dx > abs_dy * DOMINANCE {
        if dx > 0.0 {
            1 // East
        } else {
            3 // West
        }
    } else if abs_dy > abs_dx * DOMINANCE {
        if dy > 0.0 {
            2 // South
        } else {
            0 // North
        }
    } else {
        info!("alt-swipe: direction too diagonal; falling back to tap");
        engine.handle_input(EngineInput::KeyTap(source_box.key.clone()));
        return;
    };

    // Pick the alt character at this direction's slot. The convention
    // for AltKeySwipe is: extended[0] = N, [1] = E, [2] = S, [3] = W.
    // Layouts that want this should populate `extended` in that order.
    let alt = source_box.key.extended.get(direction_idx).cloned();
    match alt {
        Some(c) if !c.is_empty() => {
            info!(
                "alt-swipe: dir_idx={} → '{}' (source '{}')",
                direction_idx, c, source_box.key.label
            );
            // Route via StenoCommit, NOT SwipeWord. Alt-swipes commit
            // a SINGLE CHARACTER (the alt-key reachable via a one-
            // cell directional swipe — e.g., swipe-down on letter
            // 't' → '5'). Going through SwipeWord would attach
            // SwipeWord's auto-space semantics: a swipe-typed word
            // followed by an alt-swipe would insert a leading space
            // ('hello' + alt-5 → 'hello 5' instead of 'hello5').
            // StenoCommit commits verbatim with no spacing — exactly
            // what a single-char alt insert needs.
            engine.handle_input(EngineInput::StenoCommit(c));
        }
        _ => {
            info!(
                "alt-swipe: source '{}' has no alt for direction idx {}",
                source_box.key.label, direction_idx
            );
        }
    }
}

/// Behavior for a SwipeSource key (spacebar in springboard mode, or
/// any letter in non-cardinal layouts).
///
/// Runs the full springboard + recognizer pipeline. Also handles the
/// minimize-fling and spacebar-arrow detection, both of which only
/// make sense when the gesture started from a swipe-source (typically
/// space or a top-row letter).
///
/// Note: this is the LARGEST behavior arm because swipe-typing
/// involves dictionary lookup + segmentation across letter boundaries.
/// Cardinal / Normal / AltKeySwipe are all much simpler.
#[allow(clippy::too_many_arguments)]
fn dispatch_swipe_source(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
    engine: &KeyboardEngine,
    recognizer: &Rc<RefCell<GestureRecognizer>>,
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    current_layout: &Rc<RefCell<Layout>>,
    visible_mode: &Rc<Cell<VisibleMode>>,
    kb_box_for_tray: &gtk4::Box,
    widgets_for_tray: &Rc<RefCell<Vec<KeyWidget>>>,
    cur_layout_for_tray: &Rc<RefCell<Layout>>,
    engine_for_tray: &KeyboardEngine,
    recognizer_for_tray: &Rc<RefCell<GestureRecognizer>>,
    dx: f64,
    dy: f64,
) {
    use crate::gestures::boundary::{segment_path, LetterBoundary};

    // ===================================================================
    // SWIPE-SOURCE INTERCEPTOR PRECEDENCE
    // ===================================================================
    // This function is the freeform-swipe entry point (the SwipeSource
    // button behavior). Several gesture interceptors run in a fixed
    // precedence order; the FIRST one that matches CONSUMES the gesture
    // (early return) so nothing leaks downstream. Order:
    //
    //   1. Minimize-fling      → Full keyboard collapses to tray.
    //   2. Spacebar arrow/canvas → arrow keys, or (long up) draw canvas.
    //   3. Top-row digit swipe → short up-arc on top row emits a digit.
    //   4. Recognizer pipeline → springboard-segment + dictionary match.
    //
    // Interceptors 1-3 are SUPPRESSED entirely in experimental modes
    // (cardinal, stenotype) — those layouts route their gestures through
    // `dispatch_cardinal_swipe`, never here, but we belt-and-suspenders
    // gate anyway in case a stray SwipeSource key sneaks into one.
    //
    // The single `dispatch_mode` below is the ONE source of truth for
    // "which mode are we in"; do NOT re-derive a local is_cardinal bool.
    // The earlier duplicated derivations drifted out of sync and caused
    // the canvas-open + close-keyboard bugs.
    // ===================================================================
    let dispatch_mode = cur_layout_for_tray.borrow().style.dispatch_mode();
    let run_standard_interceptors = !dispatch_mode.suppresses_freeform_swipe();

    // (1) Minimize-fling already handled at the drag_end level
    // (it runs BEFORE the per-behavior dispatch so it works in
    // cardinal/stenotype modes too). No re-check needed here.

    // (2) Spacebar arrow-swipe + character-draw canvas entry.
    if run_standard_interceptors {
        if let Some(arrow_key) = detect_spacebar_arrow_swipe(path, boxes, dx, dy) {
            if arrow_key == "Up" {
                // Only a LONG, deliberate upward swipe opens the
                // drawing canvas. A short up-flick sends the Up
                // arrow like the other directions. Prevents the
                // canvas popping open (and shifting the whole UI)
                // on an accidental small vertical motion.
                const CANVAS_OPEN_MIN_DY: f64 = 70.0;
                if dy.abs() >= CANVAS_OPEN_MIN_DY {
                    info!("swipe-source: long spacebar swipe-up → open drawing canvas");
                    crate::keyboard::character_draw::open_canvas();
                    crate::ui::window::post_microterm_status(
                        "Drawing canvas open — draw, then ✓ to recognize or ✕ to cancel",
                    );
                    return;
                }
                info!("swipe-source: short spacebar swipe-up → Up arrow");
                engine.handle_input(EngineInput::NamedKey("Up".to_string()));
                return;
            }
            info!("swipe-source: spacebar swipe → arrow key: {}", arrow_key);
            engine.handle_input(EngineInput::NamedKey(arrow_key.to_string()));
            return;
        }
    }

    // (3) Top-row digit swipe — short upward arc on top-row keys.
    if run_standard_interceptors {
        if let Some(digit) = detect_top_row_digit(path, boxes) {
            info!("swipe-source: top-row up-swipe → digit '{}'", digit);
            // StenoCommit, not SwipeWord — see the cardinal /
            // alt-swipe rationale: a single character should not
            // participate in SwipeWord's auto-space (which would
            // turn 'hello' + top-row-swipe-to-'5' into 'hello 5').
            engine.handle_input(EngineInput::StenoCommit(digit.to_string()));
            return;
        }
    }

    // (4) Recognizer pipeline. Springboard-segment the path, then for
    // each segment ask the recognizer for the best dictionary match.
    //
    // Gate: when the user has disabled swipe-typing, skip this entire
    // pipeline. The interceptors above (spacebar arrow swipe, top-row
    // digit, minimize fling) already ran and short-circuited if they
    // matched — they're useful gestures regardless of the recognizer
    // setting. Falling through to here means the user dragged from a
    // SwipeSource without matching an interceptor; with swipe-typing
    // off they don't want a word committed, so it's a silent no-op.
    // This is what gives the user the "spacebar as arrow-pad" feel
    // when they turn swipe-typing off: arrow swipes still work, but
    // any other drag from space (or a letter key downgraded to
    // SwipeSource) does nothing rather than typing a guessed word.
    if !crate::gestures::swipe::swipe_typing_enabled() {
        debug!("swipe-source: swipe-typing disabled — no recognizer pass");
        return;
    }

    let boundary = LetterBoundary::from_hitboxes(boxes);
    let segments = if let Some(ref b) = boundary {
        segment_path(path, b)
    } else {
        // No keys → no boundary. Treat the whole path as one segment.
        vec![crate::gestures::boundary::PathSegment {
            points: path.to_vec(),
            crossed_into: None,
        }]
    };

    if segments.len() > 1 {
        info!("springboard: path segmented into {} words", segments.len());
    }

    let dispatch_mode_seg = current_layout.borrow().style.dispatch_mode();
    let mut accumulated = String::new();

    for (i, seg) in segments.iter().enumerate() {
        // In experimental modes (which shouldn't normally reach
        // SwipeSource, but just in case), only the first segment is
        // processed and through dispatch_cardinal_swipe.
        if dispatch_mode_seg.uses_cardinal_dispatch() {
            if i == 0 {
                info!("swipe-source: cardinal-style fallback dispatch (segment {})", i + 1);
                dispatch_cardinal_swipe(&seg.points, boxes, engine);
            }
            break;
        }

        refresh_recognizer_positions(boxes, recognizer);
        let avg_v = crate::gestures::swipe::path_avg_velocity(&seg.points);

        // Diagnostic: log the recognizer-pipeline INPUTS so an
        // on-device journal pull can tell whether a "line shows but
        // nothing typed" failure is empty path, no swipe_target keys,
        // zero candidates, or empty raw decode. The four scalars
        // here are each greppable and uniquely identify the failure
        // mode without needing extra debug-level enabling.
        let swipe_target_count =
            boxes.iter().filter(|b| b.key.swipe_target).count();
        info!(
            "swipe pipeline inputs: seg={} points={} boxes={} swipe_target_keys={} \
             avg_v={:.3}px/ms",
            i, seg.points.len(), boxes.len(), swipe_target_count, avg_v
        );

        let mut candidates = recognizer
            .borrow()
            .recognize_with_speed(&seg.points, boxes, avg_v);
        // AI swipe assist: re-rank the candidates using personalized
        // signals (adaptive dictionary + n-gram next-word LM).
        // No-op when the master toggle is off — costs only one
        // atomic-load + branch in that case.
        crate::gestures::ai_swipe::apply_to_candidates(&mut candidates);
        let raw = crate::gestures::swipe::decode_swipe_path(&seg.points, boxes);
        // Diagnostic: candidate count + raw decode. If candidates is
        // 0 AND raw is "" the failure is the decode (no swipe_target
        // letters near pivots). If candidates is 0 but raw has
        // letters, the dictionary didn't match anything close enough.
        info!(
            "swipe pipeline output: candidates={} raw={:?}",
            candidates.len(), raw
        );
        let chosen = candidates
            .into_iter()
            .next()
            .map(|c| c.word)
            .unwrap_or(raw);
        info!(
            "swipe segment {}: avg_v={:.2}px/ms chosen={:?}",
            i, avg_v, chosen
        );
        if chosen.is_empty()
            && crate::gestures::cardinal::cardinal_diagnostic_enabled()
        {
            // Diagnostic visible on-device. If chosen is empty, the
            // recognizer found no dictionary candidate AND the raw
            // pivot extraction returned nothing. Usual cause: path
            // had too few pivots (very short swipe), OR no key hit
            // any pivot, OR the dictionary is empty/not loaded.
            crate::ui::window::post_microterm_status(format!(
                "swipe segment {}: nothing decoded (path={} points)",
                i, seg.points.len()
            ));
        }
        if !chosen.is_empty() {
            // Record the commit BEFORE accumulating + before the
            // next segment runs. This way segment 2's
            // apply_to_candidates can use segment 1's word as the
            // n-gram `prev`. Pre-fix, all record_commit calls fired
            // AFTER the loop, so within a single springboard swipe
            // (e.g. "thank you very much"), segments 2+ couldn't
            // benefit from earlier segments as context — missing
            // the most valuable disambiguation case (later words in
            // a phrase given earlier ones).
            crate::gestures::ai_swipe::record_commit(&chosen);
            if !accumulated.is_empty() && !accumulated.ends_with(' ') {
                // Avoid double-spacing: the post-crossing block
                // below already appended ' ' after the previous
                // segment if it crossed into the spacebar zone.
                // Without this guard, multi-segment springboards
                // produce "wordA  wordB" with double spaces.
                accumulated.push(' ');
            }
            accumulated.push_str(&chosen);
        }
        // Springboard edge crossing → append a space + pulse spacebar.
        if seg.crossed_into.is_some() && !accumulated.ends_with(' ') {
            accumulated.push(' ');
            flash_spacebar(widgets);
        }
    }

    if !accumulated.is_empty() {
        // ai_swipe::record_commit is called PER SEGMENT in the loop
        // above (so segment 2's n-gram context reflects segment 1's
        // word). Don't double-record here.
        info!(
            "swipe pipeline → SwipeWord: segments={} accumulated={:?}",
            segments.len(),
            accumulated
        );
        engine.handle_input(EngineInput::SwipeWord(accumulated));
        // Final-commit feedback: even a single-word swipe (no
        // springboard crossing) gets the accent glow so the user
        // sees the word landed.
        flash_spacebar(widgets);
    } else {
        // The recognizer pipeline produced no text from ANY segment.
        // Definitive signal that "trail rendered but nothing typed."
        // Logged at warn! because it's a user-visible failure mode
        // we want surfaced without RUST_LOG tweaks. The per-segment
        // diagnostic above already showed the inputs (points,
        // boxes, swipe_target_keys, raw, candidates); this line
        // confirms that NONE of them yielded a word.
        tracing::warn!(
            "swipe pipeline → no word committed: segments={} \
             path_points={} all chosen empty. Trail rendered but no \
             SwipeWord fired.",
            segments.len(),
            path.len()
        );
    }
}

/// Flash the spacebar as positive word-commit feedback: the opacity
/// press-pulse plus the accent-glow commit-flash. Locates the
/// spacebar widget in the current widget set; no-op if absent (e.g.
/// experimental layouts without a space key).
fn flash_spacebar(widgets: &Rc<RefCell<Vec<KeyWidget>>>) {
    for w in widgets.borrow().iter() {
        if matches!(w.key().action, KeyAction::Space) {
            w.animate_press();
            crate::ui::animations::Animator::commit_flash(w.widget());
            break;
        }
    }
}

/// Maximum rect-distance for the nearest-cell fallback in
/// `find_cardinal_source`. A touch this close to a cardinal cell's
/// boundary (but technically outside every widget's bounds, e.g. in a
/// CSS row gap) still resolves to that cell. Larger and stray touches
/// in the chrome/padding would grab unrelated cells.
const NEAREST_CARDINAL_RECT_PX: f64 = 10.0;

/// Resolve which cardinal cell a swipe started from. Strict
/// `contains()` first (consistent with drag_begin and every other
/// hit-test), then a nearest-cell fallback within
/// `NEAREST_CARDINAL_RECT_PX` for touches that land in inter-cell
/// gaps. Factored out so the dispatcher AND the gesture classifier
/// share ONE implementation — duplicating it risks the same drift
/// that caused the earlier strict-vs-contains bug.
fn find_cardinal_source<'a>(
    start: &crate::gestures::swipe::SwipePoint,
    boxes: &'a [KeyHitbox],
) -> Option<&'a KeyHitbox> {
    if let Some(b) = boxes.iter().find(|b| b.contains(start.x, start.y)) {
        return Some(b);
    }
    boxes
        .iter()
        .filter(|b| b.key.gestures.is_some())
        .map(|b| {
            let nx = start.x.clamp(b.x, b.x + b.width);
            let ny = start.y.clamp(b.y, b.y + b.height);
            let dx = start.x - nx;
            let dy = start.y - ny;
            (b, (dx * dx + dy * dy).sqrt())
        })
        .filter(|(_, d)| *d <= NEAREST_CARDINAL_RECT_PX)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(b, _)| b)
}

/// Translate a path into the source cell's local frame (cell center →
/// origin) and classify it. Shared by the dispatcher and the
/// classifier predicate so both see the same direction interpretation.
fn classify_cardinal_in_local_frame(
    path: &[crate::gestures::swipe::SwipePoint],
    source_box: &KeyHitbox,
) -> crate::gestures::cardinal::CardinalGesture {
    use crate::gestures::cardinal::{classify, CardinalThresholds};
    let cx = source_box.x + source_box.width / 2.0;
    let cy = source_box.y + source_box.height / 2.0;
    let local_path: Vec<crate::gestures::swipe::SwipePoint> = path
        .iter()
        .map(|p| crate::gestures::swipe::SwipePoint {
            x: p.x - cx,
            y: p.y - cy,
            timestamp_ms: p.timestamp_ms,
        })
        .collect();
    let thresholds = if crate::gestures::cardinal::cardinal_lenient() {
        CardinalThresholds::lenient()
    } else {
        CardinalThresholds::default()
    };
    classify(&local_path, &thresholds)
}

/// Side-effect-free predicate: would `dispatch_cardinal_swipe` commit
/// something for this path? Used by `classify_drag_gesture` so a valid
/// cardinal gesture is categorized as `DragGesture::Cardinal` and
/// cannot be overruled by an earlier cross-behavior detector
/// (minimize-fling, bottom-pad arrow). Mirrors the dispatcher's
/// resolution exactly via the shared helpers above, minus the engine
/// dispatch and logging.
fn would_resolve_as_cardinal(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
) -> bool {
    use crate::gestures::cardinal::{dispatch, CardinalGesture, Direction};
    let Some(start) = path.first() else {
        return false;
    };
    let Some(source_box) = find_cardinal_source(start, boxes) else {
        return false;
    };
    let key = &source_box.key;
    let Some(gestures) = key.gestures.as_ref() else {
        return false;
    };
    let classified = classify_cardinal_in_local_frame(path, source_box);
    if matches!(classified, CardinalGesture::Tap | CardinalGesture::Unrecognized) {
        return false;
    }
    // Steno cells resolve only for the four cardinal directions (the
    // dispatcher ignores diagonals/hairpins/rotations for steno).
    if matches!(key.action, KeyAction::Steno(_)) {
        return matches!(
            classified,
            CardinalGesture::Cardinal(
                Direction::N | Direction::E | Direction::S | Direction::W
            )
        );
    }
    // Non-steno: resolves iff the gesture maps to a non-empty action.
    matches!(dispatch(gestures, classified), Some(t) if !t.is_empty())
}

fn dispatch_cardinal_swipe(
    path: &[crate::gestures::swipe::SwipePoint],
    boxes: &[KeyHitbox],
    engine: &KeyboardEngine,
) {
    use crate::gestures::cardinal::{dispatch, CardinalGesture};

    let Some(start) = path.first() else {
        return;
    };

    let source = find_cardinal_source(start, boxes);
    let Some(source_box) = source else {
        info!(
            "cardinal swipe: start point ({:.1}, {:.1}) didn't hit any key (boxes.len={})",
            start.x, start.y, boxes.len()
        );
        // Diagnostic visible on-device: tells the user the swipe
        // pipeline RAN but couldn't resolve which key to dispatch
        // for. With the nearest-cell fallback above, this only
        // fires when start was >30px from every cardinal cell —
        // i.e. genuinely off the keyboard, not just in a gap.
        if crate::gestures::cardinal::cardinal_diagnostic_enabled() {
            crate::ui::window::post_microterm_status(format!(
                "swipe at ({:.0},{:.0}) hit no key (hitboxes={})",
                start.x, start.y, boxes.len()
            ));
        }
        return;
    };
    let key = &source_box.key;

    let Some(gestures) = key.gestures.as_ref() else {
        info!(
            "cardinal swipe: source key '{}' has no gesture map; ignoring",
            key.label
        );
        if crate::gestures::cardinal::cardinal_diagnostic_enabled() {
            crate::ui::window::post_microterm_status(format!(
                "swipe on '{}' but key has no gesture map",
                key.label
            ));
        }
        return;
    };

    // Translate path into the source cell's local frame and classify.
    // Shared helper keeps this identical to the classifier predicate.
    let classified = classify_cardinal_in_local_frame(path, source_box);

    info!(
        "cardinal swipe on '{}': classified as {:?}",
        key.label, classified
    );

    if matches!(classified, CardinalGesture::Tap | CardinalGesture::Unrecognized) {
        // Tap is handled by the KeyWidget's own click handler; we'd
        // double-fire if we forwarded it here. Unrecognized → drop.
        return;
    }

    // Steno cardinal swipe → feed the steno engine, bypassing the
    // normal text-commit path. The label rendered on the cardinal
    // direction is the affordance; the actual phoneme comes from
    // the steno engine's table lookup. This way the user sees the
    // visualizer's behavior in the real keyboard.
    if let KeyAction::Steno(button_id) = &key.action {
        if let Some(button) = crate::steno::StenoButton::parse(button_id) {
            let steno_gesture = match classified {
                CardinalGesture::Cardinal(crate::gestures::cardinal::Direction::N) =>
                    Some(crate::steno::StenoGesture::SwipeN),
                CardinalGesture::Cardinal(crate::gestures::cardinal::Direction::E) =>
                    Some(crate::steno::StenoGesture::SwipeE),
                CardinalGesture::Cardinal(crate::gestures::cardinal::Direction::S) =>
                    Some(crate::steno::StenoGesture::SwipeS),
                CardinalGesture::Cardinal(crate::gestures::cardinal::Direction::W) =>
                    Some(crate::steno::StenoGesture::SwipeW),
                _ => None, // Hairpin/rotation/diagonals: not used by steno
            };
            if let Some(g) = steno_gesture {
                handle_steno_gesture(engine, button, g);
            } else {
                // User did a diagonal, hairpin, or rotation — none
                // of which map to a steno gesture in v0. Surface a
                // hint so they know what happened.
                info!(
                    "steno: ignored non-cardinal gesture {:?} on {}",
                    classified, button_id
                );
                crate::ui::window::post_microterm_status(
                    "Steno: only tap and N/E/S/W swipes are mapped",
                );
            }
            return;
        }
    }

    match dispatch(gestures, classified) {
        Some(text) if !text.is_empty() => {
            // Source-key-action-aware routing. Keysym sources are
            // joystick-style directional pads where each "gesture
            // text" is actually a keysym NAME ("Up", "Down", etc.) —
            // emit them as NamedKey so the virtual keyboard backend
            // sends the matching X keysym press, not the literal
            // string "Up". Character sources (cardinal-grid cells
            // and steno) keep the StenoCommit path that types the
            // characters directly.
            if matches!(key.action, KeyAction::Keysym(_)) {
                info!(
                    "cardinal swipe (joystick): emitting NamedKey {:?}",
                    text
                );
                engine.handle_input(EngineInput::NamedKey(text));
            } else {
                info!("cardinal swipe: committing {:?}", text);
                // Route via StenoCommit, NOT SwipeWord. Cardinal swipes
                // emit a SINGLE CHARACTER per gesture (e.g. 'u' for
                // north on cell 'a'); SwipeWord's auto-space semantics
                // would treat each one as "just finished a word" and
                // prepend a space to the next, producing 'u e' instead
                // of 'ue'. StenoCommit commits verbatim and resets the
                // swipe-sequence flag — exactly what we want here.
                // (Name is historical — same primitive serves any
                // verbatim short-text commit that shouldn't participate
                // in inter-swipe auto-spacing.)
                engine.handle_input(EngineInput::StenoCommit(text));
            }
        }
        Some(_) => {
            info!("cardinal swipe: gesture mapped to empty string; dropping");
        }
        None => {
            // Surface as info! so it shows on-device under the default
            // log filter; the most common cause for "swipe didn't type
            // anything" is a missing mapping on the source key.
            info!(
                "cardinal swipe: key '{}' has no mapping for gesture {:?}",
                key.label, classified
            );
            if crate::gestures::cardinal::cardinal_diagnostic_enabled() {
                crate::ui::window::post_microterm_status(format!(
                    "swipe on '{}' → {:?} but no mapping",
                    key.label, classified
                ));
            }
        }
    }
}

/// Show the in-keyboard settings panel by replacing the contents of
/// `keyboard_box` with a `SettingsPanel`. When the user clicks "Done"
/// or changes any setting that affects the layout, rebuild the
/// keyboard view from the new config.
///
/// We swap children of `keyboard_box` rather than overlay-stacking
/// because the settings panel wants the full keyboard area and we'd
/// otherwise have to manage pointer-event opacity.
fn show_settings_panel(
    keyboard_box: &gtk4::Box,
    current_layout: &Rc<RefCell<Layout>>,
    widgets: &Rc<RefCell<Vec<KeyWidget>>>,
    engine: &KeyboardEngine,
    recognizer: &Rc<RefCell<GestureRecognizer>>,
    cfg: &Config,
    trail: &SwipeTrail,
    last_letter_layout: &Rc<RefCell<Layout>>,
) {
    use crate::ui::settings_panel::SettingsPanel;
    info!("Opening settings panel");

    // Opening settings is a meaningful editing boundary: any in-flight
    // voice transcription or LLM request belongs to the field the user
    // just left. Cancel them so a late-arriving response doesn't
    // commit into the field after the user closes settings. The
    // microterm content also belongs to that field — its chips would
    // route taps back to the engine (and from there to the underlying
    // host field, which is now hidden under settings), so a stray tap
    // could corrupt that field. Hide the microterm here for the same
    // reason we do on focus change.
    crate::ui::window::request_microterm_cancel();
    // Drop any undrained steno suggestion in the slot so the pump
    // can't surface it over the settings panel after we hide the
    // microterm.
    crate::ui::window::clear_steno_suggestions();
    // Note: we don't have the microterm handle in scope here, but
    // the in-flight bridges (voice, LLM) react to cancel_generation
    // by clearing their own microterm content. Steno suggestions
    // self-hide on a 5s timer; since settings closes back to letter
    // layout (not steno), this is acceptable for now.

    // Clear the keyboard area.
    while let Some(child) = keyboard_box.first_child() {
        keyboard_box.remove(&child);
    }
    widgets.borrow_mut().clear();

    // Settings is text-heavy and the user shouldn't have to scroll a
    // tiny window on phone. Grow the layer surface upward so the panel
    // gets the vertical room it needs. The on_done callback will reset
    // height to the layout's natural value.
    const SETTINGS_HEIGHT_PX: i32 = 560;
    DESIRED_KEYBOARD_HEIGHT.store(SETTINGS_HEIGHT_PX, std::sync::atomic::Ordering::Relaxed);
    if let Some(root) = keyboard_box.root() {
        if let Some(win) = root.dynamic_cast_ref::<gtk4::Window>() {
            win.set_default_height(SETTINGS_HEIGHT_PX);
        }
    }

    // Build the settings panel with two callbacks:
    //
    //   on_change: fires on every toggle. Updates the cached layout,
    //              reinstalls CSS for live color/compact changes, and
    //              persists the config. Does NOT touch the panel.
    //
    //   on_done:   fires once when the user taps Done. Tears down the
    //              panel and rebuilds the keyboard.
    //
    // The previous design used a single callback for both, which made
    // every toggle close the settings page — a real UX disaster the
    // user explicitly flagged. Splitting fixes that.
    let kb_box = keyboard_box.clone();
    let cur_layout = current_layout.clone();
    let widgets_clone = widgets.clone();
    let engine_clone = engine.clone();
    let recognizer_clone = recognizer.clone();
    let trail_for_change = trail.clone();
    let last_letter_for_done = last_letter_layout.clone();

    // on_change — live updates without closing.
    let on_change: crate::ui::settings_panel::SettingsChangeCallback = {
        let cur_layout = cur_layout.clone();
        Rc::new(move |new_cfg| {
            info!(
                "settings live: letters={}, row_style={}, cardinal={}, compact={}, single_hand={}",
                new_cfg.layout.letters,
                new_cfg.layout.row_style,
                new_cfg.layout.cardinal_enabled,
                new_cfg.layout.compact,
                new_cfg.layout.thumb_arc.is_some()
            );
            // Cache the resolved layout for when Done is hit.
            let new_layout = resolve_letter_layout(new_cfg);
            let new_h = desired_height_for(new_layout.rows.len(), new_cfg.layout.compact);
            DESIRED_KEYBOARD_HEIGHT.store(new_h, std::sync::atomic::Ordering::Relaxed);
            *cur_layout.borrow_mut() = new_layout;
            // Push the thumb-arc into the process-wide snapshot so
            // compute_row_geometry sees it on the next rebuild.
            *CURRENT_THUMB_ARC.lock() = new_cfg.layout.thumb_arc.clone();
            set_width_scale(new_cfg.layout.width_scale);
            set_height_scale(new_cfg.layout.height_scale);
            set_bottom_pad(new_cfg.layout.bottom_pad_px);
            set_compact_mode(new_cfg.layout.compact);
            // Push key-repeat config so subsequent presses use the
            // updated enable/intervals immediately.
            *KEY_REPEAT_CONFIG.lock() = KeyRepeatConfig {
                enabled: new_cfg.behavior.key_repeat,
                delay_ms: new_cfg.behavior.repeat_delay_ms,
                interval_ms: new_cfg.behavior.repeat_interval_ms,
            };
            // Push the swipe trail color through directly — the trail
            // widget doesn't watch the CSS provider, so we have to set
            // its color manually. Prefer the explicit trail pick over
            // the legacy accent_color.
            let trail_hex = if !new_cfg.appearance.colors.trail.is_empty() {
                new_cfg.appearance.colors.trail.clone()
            } else {
                new_cfg.appearance.accent_color.clone()
            };
            trail_for_change.set_color_hex(&trail_hex);
            // Reinstall CSS so color picker changes + compact mode
            // take effect immediately. The settings panel itself stays
            // open — we don't touch kb_box's children here.
            crate::ui::theme::install_css_provider(&new_cfg.appearance, &new_cfg.layout);
        })
    };

    // on_done — clear panel, build keys.
    let on_done: crate::ui::settings_panel::SettingsDoneCallback = Rc::new(move |new_cfg| {
        info!("settings done: closing panel, rebuilding keyboard");
        if let Err(e) = new_cfg.save() {
            tracing::warn!("failed to persist config on done: {}", e);
        }
        // Settings can be opened from a transient page (Symbols /
        // Numbers), which leaves the engine's keypad state stuck on
        // that page. on_done always rebuilds to the LETTER layout, so
        // reset the engine state to Normal too — otherwise the engine
        // believes it's in Symbols while the view shows letters,
        // desyncing shift/auto-cap (the next shift evaluation would
        // operate on the wrong state). set_keypad_state doesn't emit
        // an event, so this won't re-trigger the StateChanged handler.
        engine_clone.set_keypad_state(KeypadState::Normal);
        // Settings is a meaningful editing boundary: the user may have
        // changed keyboard mode (e.g. steno → QWERTY), which would
        // otherwise leave a stale steno phoneme buffer that silently
        // re-activates the next time steno is re-entered, pairing
        // against zombie state. Clear it here. Also reset the swipe-
        // sequence flag so a following swipe doesn't auto-space after
        // the panel close.
        crate::steno::global().clear_buffer();
        // Restore the natural keyboard height now that we're going back
        // from the tall settings page to the regular key grid.
        let new_layout = resolve_letter_layout(new_cfg);
        let target_h = desired_height_for(new_layout.rows.len(), new_cfg.layout.compact);
        DESIRED_KEYBOARD_HEIGHT.store(target_h, std::sync::atomic::Ordering::Relaxed);
        *cur_layout.borrow_mut() = new_layout.clone();
        // Settings-driven layout change must also update the
        // letter-stash so subsequent symbols-page round-trips
        // restore the new layout, not the old one. Pre-fix, a
        // qwerty→dvorak switch via settings, then symbols toggle,
        // then back to letters would restore to qwerty (the
        // stash pointed at the pre-settings layout) — the user
        // expected dvorak.
        *last_letter_for_done.borrow_mut() = new_layout;
        while let Some(child) = kb_box.first_child() {
            kb_box.remove(&child);
        }
        widgets_clone.borrow_mut().clear();
        rebuild_view(
            &kb_box,
            &widgets_clone,
            &cur_layout,
            &engine_clone,
            &recognizer_clone,
        );
    });

    let panel = SettingsPanel::new(cfg.clone(), on_change, on_done);
    keyboard_box.append(panel.widget());
}

/// Dump a table of per-layout, per-screen-width row geometry to
/// stdout for diagnostic purposes. Called from `main()` when the
/// `--debug-layout` flag is passed. Useful for verifying on a
/// specific device that no row would overflow at that device's
/// reported screen width.
///
/// Output format: a header row + one row per (combo, screen_w, layout_row).
/// Columns: combo description, screen_w, row index, unit_w, predicted
/// row width, screen budget, status (OK/OVER).
///
/// No GTK calls — purely the same math the test suite uses.
pub fn debug_audit_layouts() {
    use crate::config::{Config, ThumbAnchor, ThumbArc};

    fn make_cfg(
        letters: &str,
        row_style: &str,
        cardinal_enabled: bool,
        compact: bool,
        thumb_anchor: Option<ThumbAnchor>,
    ) -> Config {
        let mut cfg = Config::default();
        cfg.layout.letters = letters.into();
        cfg.layout.row_style = row_style.into();
        cfg.layout.cardinal_enabled = cardinal_enabled;
        cfg.layout.experimental_mode =
            if cardinal_enabled { "cardinal".into() } else { "none".into() };
        cfg.layout.compact = compact;
        cfg.layout.thumb_arc = thumb_anchor.map(|a| ThumbArc::default_for_width(a, 400.0));
        cfg
    }

    let combos: Vec<(&str, &str, bool, bool, Option<ThumbAnchor>)> = vec![
        ("qwerty", "staggered", false, false, None),
        ("qwerty", "staggered", false, true, None),
        ("qwerty", "ortholinear", false, false, None),
        ("qwerty", "ortholinear", false, true, None),
        ("dvorak", "staggered", false, false, None),
        ("dvorak", "staggered", false, true, None),
        ("dvorak", "ortholinear", false, false, None),
        ("dvorak", "ortholinear", false, true, None),
        ("qwerty", "ortholinear", true, false, None),
        ("dvorak", "ortholinear", true, false, None),
        ("qwerty", "ortholinear", false, false, Some(ThumbAnchor::BottomRight)),
        ("qwerty", "ortholinear", false, false, Some(ThumbAnchor::BottomLeft)),
        ("dvorak", "ortholinear", true, false, Some(ThumbAnchor::BottomRight)),
    ];
    let screens: &[i32] = &[360, 400, 720, 1280];

    println!(
        "{:<54} | {:>7} | {:>3} | {:>5} | {:>9} | {:>7} | {}",
        "combo", "screen", "row", "unit", "row_width", "budget", "status"
    );
    println!("{}", "-".repeat(108));

    for (letters, row_style, cardinal, compact, anchor) in combos {
        let cfg = make_cfg(letters, row_style, cardinal, compact, anchor);
        let layout = resolve_letter_layout(&cfg);
        let combo_desc = format!(
            "{}/{}/cardinal={}/compact={}/anchor={:?}",
            letters, row_style, cardinal, compact, anchor
        );

        for &screen_w in screens {
            let unit_w = compute_unit_width_from_screen_w(
                screen_w,
                layout.grid_columns,
                &layout.rows,
                layout.ortholinear,
            );
            for (row_idx, row) in layout.rows.iter().enumerate() {
                let total_units: f64 = row.keys.iter().map(|k| k.width as f64).sum();
                let key_count = row.keys.len() as i32;
                let leading_pad_px = if layout.ortholinear || row.offset <= 0.0 {
                    0.0
                } else {
                    row.offset as f64 * unit_w as f64
                };
                let spacing_px = 4.0 * (key_count - 1).max(0) as f64;
                let row_width_px = leading_pad_px + total_units * unit_w as f64 + spacing_px;
                let budget = (screen_w - 16) as f64;
                let status = if row_width_px <= budget + 1.0 {
                    "OK"
                } else {
                    "OVER"
                };
                println!(
                    "{:<54} | {:>7} | {:>3} | {:>5} | {:>9.1} | {:>7.1} | {}",
                    combo_desc, screen_w, row_idx, unit_w, row_width_px, budget, status
                );
            }
        }
    }
}

#[cfg(test)]
mod layout_overflow_tests {
    //! Validates that every supported layout configuration's rows fit
    //! within their allotted screen width, across phone-typical and
    //! desktop-typical screen sizes.
    //!
    //! These tests don't construct any GTK widgets — they only call
    //! the pure math (`compute_unit_width_from_screen_w`,
    //! `polygon_x_range`) and verify row-width sums against the screen.
    //! Run with `cargo test layout_overflow`.

    use super::*;
    use crate::config::{Config, ThumbAnchor, ThumbArc};

    /// Screen widths to test in logical px:
    ///   360 — typical phone at 300% scaling (Plasma Mobile FP5)
    ///   400 — small tablet / large phone landscape
    ///   720 — small laptop window
    ///   1280 — desktop maximised
    const TEST_SCREEN_WIDTHS: &[i32] = &[360, 400, 720, 1280];

    fn make_cfg(
        letters: &str,
        row_style: &str,
        cardinal_enabled: bool,
        compact: bool,
        thumb_anchor: Option<ThumbAnchor>,
    ) -> Config {
        let mut cfg = Config::default();
        cfg.layout.letters = letters.into();
        cfg.layout.row_style = row_style.into();
        cfg.layout.cardinal_enabled = cardinal_enabled;
        cfg.layout.experimental_mode =
            if cardinal_enabled { "cardinal".into() } else { "none".into() };
        cfg.layout.compact = compact;
        cfg.layout.thumb_arc = thumb_anchor.map(|a| ThumbArc::default_for_width(a, 400.0));
        cfg
    }

    /// Verifies that for every combination, the resolved layout's
    /// widest row fits inside the screen-width budget after accounting
    /// for the unit_w returned by `compute_unit_width_from_screen_w`.
    #[test]
    fn no_row_overflows_default_screens() {
        let combos: Vec<(&str, &str, bool, bool, Option<ThumbAnchor>)> = vec![
            // (letters, row_style, cardinal, compact, thumb_anchor)
            ("qwerty", "staggered", false, false, None),
            ("qwerty", "staggered", false, true, None),
            ("qwerty", "ortholinear", false, false, None),
            ("qwerty", "ortholinear", false, true, None),
            ("dvorak", "staggered", false, false, None),
            ("dvorak", "staggered", false, true, None),
            ("dvorak", "ortholinear", false, false, None),
            ("dvorak", "ortholinear", false, true, None),
            ("qwerty", "ortholinear", true, false, None),
            ("dvorak", "ortholinear", true, false, None),
            // Thumb-arc variants (only test ortho, since the arc
            // overrides leading pad and stagger is moot under it).
            ("qwerty", "ortholinear", false, false, Some(ThumbAnchor::BottomRight)),
            ("qwerty", "ortholinear", false, false, Some(ThumbAnchor::BottomLeft)),
            ("dvorak", "ortholinear", true, false, Some(ThumbAnchor::BottomRight)),
        ];

        for (letters, row_style, cardinal, compact, anchor) in combos {
            let cfg = make_cfg(letters, row_style, cardinal, compact, anchor);
            let layout = resolve_letter_layout(&cfg);

            for &screen_w in TEST_SCREEN_WIDTHS {
                let unit_w = compute_unit_width_from_screen_w(
                    screen_w,
                    layout.grid_columns,
                    &layout.rows,
                    layout.ortholinear,
                );

                // Verify each row's predicted total width fits.
                for (row_idx, row) in layout.rows.iter().enumerate() {
                    let total_units: f64 =
                        row.keys.iter().map(|k| k.width as f64).sum();
                    let key_count = row.keys.len() as i32;
                    let leading_pad_px = if layout.ortholinear || row.offset <= 0.0 {
                        0.0
                    } else {
                        row.offset as f64 * unit_w as f64
                    };
                    let spacing_px = 6.0 * (key_count - 1).max(0) as f64;
                    let row_width_px =
                        leading_pad_px + total_units * unit_w as f64 + spacing_px;

                    // Allow up to OUTER_MARGIN of slack (the keyboard_box
                    // has margins that aren't part of the per-key budget).
                    let budget = (screen_w - 40) as f64;
                    assert!(
                        row_width_px <= budget + 1.0,
                        "Row {} of {} ({}/{}/cardinal={}/compact={}/anchor={:?}) \
                         overflows at screen_w={}: row_width={:.1}px, budget={}px, \
                         unit_w={}",
                        row_idx,
                        layout.name,
                        letters,
                        row_style,
                        cardinal,
                        compact,
                        anchor,
                        screen_w,
                        row_width_px,
                        budget,
                        unit_w,
                    );
                }
            }
        }
    }

    /// Verifies the polygon-x-range scanline returns sensible bounds
    /// for the default thumb-arc shapes. The arc should cover the full
    /// bottom row and shrink toward the anchor for higher rows.
    #[test]
    fn polygon_x_range_default_arcs() {
        for anchor in [ThumbAnchor::BottomRight, ThumbAnchor::BottomLeft] {
            for &width in &[360.0, 480.0, 720.0] {
                let arc = ThumbArc::default_for_width(anchor, width);
                // Bottom row (y_norm ≈ 0.875): expect wide range.
                let bottom = polygon_x_range(&arc.polygon, 0.875)
                    .expect("polygon should cover bottom row y");
                let bottom_w = bottom.1 - bottom.0;
                assert!(
                    bottom_w > 0.5,
                    "Bottom row arc width too narrow at anchor={:?} w={}: {:.2}",
                    anchor,
                    width,
                    bottom_w
                );
                // Top row (y_norm ≈ 0.125): expect narrower than bottom.
                if let Some(top) = polygon_x_range(&arc.polygon, 0.125) {
                    let top_w = top.1 - top.0;
                    assert!(
                        top_w <= bottom_w + 1e-9,
                        "Top row should be no wider than bottom: \
                         anchor={:?} w={} top={:.2} bottom={:.2}",
                        anchor,
                        width,
                        top_w,
                        bottom_w
                    );
                }
            }
        }
    }

    /// Sanity check: `compute_unit_width_from_screen_w` returns a
    /// minimum unit width of 20 even with absurd inputs, and the
    /// minimum-unit clamp doesn't lead to silent overflow downstream.
    #[test]
    fn unit_width_floor_clamps() {
        let layout = Layout::default_qwerty();
        let unit_w = compute_unit_width_from_screen_w(
            100, // unrealistically narrow
            layout.grid_columns,
            &layout.rows,
            layout.ortholinear,
        );
        assert!(unit_w >= 20, "unit_w should clamp to >=20, got {}", unit_w);
    }
}

#[cfg(test)]
mod gesture_dispatch_tests {
    //! Validates the pure gesture-classification helpers and the
    //! layout→dispatch-mode mapping. No GTK widgets constructed.
    //! Run with `cargo test gesture_dispatch`.

    use super::*;
    use crate::layouts::{DispatchMode, LayoutStyle};

    #[test]
    fn arrow_swipe_below_min_length_is_none() {
        // A 20px movement is below the 28px MIN_LEN_PX threshold.
        assert_eq!(classify_arrow_swipe(20.0, 0.0), None);
        assert_eq!(classify_arrow_swipe(0.0, -20.0), None);
        assert_eq!(classify_arrow_swipe(14.0, 14.0), None); // len ~19.8
    }

    #[test]
    fn arrow_swipe_cardinal_directions() {
        // Clear horizontal swipes (well past min length, dominant axis).
        assert_eq!(classify_arrow_swipe(50.0, 0.0), Some("Right"));
        assert_eq!(classify_arrow_swipe(-50.0, 0.0), Some("Left"));
        // Vertical: screen coords, dy>0 is downward.
        assert_eq!(classify_arrow_swipe(0.0, 50.0), Some("Down"));
        assert_eq!(classify_arrow_swipe(0.0, -50.0), Some("Up"));
    }

    #[test]
    fn arrow_swipe_diagonal_is_none() {
        // 45° swipe: neither axis beats the other by the 1.5
        // dominance factor, so it must NOT fire an arrow.
        assert_eq!(classify_arrow_swipe(40.0, 40.0), None);
        assert_eq!(classify_arrow_swipe(40.0, -40.0), None);
        assert_eq!(classify_arrow_swipe(-40.0, 40.0), None);
    }

    #[test]
    fn arrow_swipe_dominance_boundary() {
        // dx must exceed dy * 1.5 to count as horizontal. At exactly
        // the boundary (dx == dy*1.5) the strict `>` should reject.
        // dy = 30, dx = 45 → 45 > 30*1.5=45 is FALSE → None.
        assert_eq!(classify_arrow_swipe(45.0, 30.0), None);
        // Just past the boundary → fires.
        assert_eq!(classify_arrow_swipe(46.0, 30.0), Some("Right"));
    }

    // ---- detect_bottom_pad_arrow regression tests ----
    //
    // The bottom-pad cursor-nav swipe lets the user move the text
    // cursor by dragging in the empty space below the keys
    // (cfg.layout.bottom_pad_px > 0). Originally only Left/Right
    // were supported; the user reported "cursor Y doesn't change in
    // multi-pane editors" because Up/Down were explicitly filtered
    // out by an `arrow == "Left" || arrow == "Right"` guard. The
    // fix added Up/Down support and relaxed the end-position
    // constraint for vertical swipes (which geometrically can't
    // stay in the pad). These tests pin the new behavior.

    fn fake_keys_with_bottom(y_bottom: f64) -> Vec<KeyHitbox> {
        use crate::keyboard::Key;
        // A single fake key at y=0 with height=y_bottom, so the
        // function's max_key_bottom calc yields y_bottom.
        vec![KeyHitbox {
            key: Key::character("a"),
            x: 0.0,
            y: 0.0,
            width: 60.0,
            height: y_bottom,
        }]
    }

    fn pt(x: f64, y: f64) -> crate::gestures::swipe::SwipePoint {
        crate::gestures::swipe::SwipePoint::new(x, y)
    }

    /// THE BUG FIX: Down swipe from pad now fires Down arrow.
    /// Pre-fix this returned None because the inner filter only
    /// matched Left/Right.
    #[test]
    fn bottom_pad_down_swipe_fires_down_arrow() {
        let boxes = fake_keys_with_bottom(100.0);
        // Start in pad (y=120 > 100), end below pad (y=180).
        // dx=0, dy=+60 → Down.
        let result = detect_bottom_pad_arrow(
            &pt(100.0, 120.0),
            &pt(100.0, 180.0),
            &boxes,
            0.0,
            60.0,
        );
        assert_eq!(result, Some("Down"));
    }

    /// THE BUG FIX: Up swipe from pad now fires Up arrow.
    /// Note the end is ABOVE the pad (in the key area, y=60 < 100).
    /// The vertical-direction relaxation lets this through; before
    /// the fix the end.y constraint would have failed even if the
    /// L/R filter had passed.
    #[test]
    fn bottom_pad_up_swipe_fires_up_arrow_even_when_end_exits_pad() {
        let boxes = fake_keys_with_bottom(100.0);
        // Start in pad (y=120), end above pad in keys area (y=60).
        // dx=0, dy=-60 → Up.
        let result = detect_bottom_pad_arrow(
            &pt(100.0, 120.0),
            &pt(100.0, 60.0),
            &boxes,
            0.0,
            -60.0,
        );
        assert_eq!(result, Some("Up"));
    }

    /// Left/Right still work as before (regression guard).
    #[test]
    fn bottom_pad_horizontal_swipes_still_work() {
        let boxes = fake_keys_with_bottom(100.0);
        // Right: stays roughly in pad area (end.y = 115).
        assert_eq!(
            detect_bottom_pad_arrow(
                &pt(100.0, 120.0),
                &pt(160.0, 115.0),
                &boxes,
                60.0,
                -5.0,
            ),
            Some("Right")
        );
        // Left: same, opposite direction.
        assert_eq!(
            detect_bottom_pad_arrow(
                &pt(160.0, 120.0),
                &pt(100.0, 115.0),
                &boxes,
                -60.0,
                -5.0,
            ),
            Some("Left")
        );
    }

    /// FALSE-POSITIVE GUARD: a HORIZONTAL swipe starting in pad but
    /// ending high in the keys area (e.g. a diagonal swipe-typing
    /// gesture that briefly dipped into the pad) must NOT fire the
    /// arrow. The end.y check only applies to horizontals.
    #[test]
    fn bottom_pad_horizontal_exiting_pad_upward_rejected() {
        let boxes = fake_keys_with_bottom(100.0);
        // Start in pad (y=120), end well above pad (y=50) — the
        // gesture left the pad zone. classify dominance: dy/dx =
        // 70/30 → vertical dominates → classify returns Up.
        // Because the direction is VERTICAL, the end.y constraint
        // doesn't apply and the gesture fires successfully.
        let result = detect_bottom_pad_arrow(
            &pt(100.0, 120.0),
            &pt(130.0, 50.0),
            &boxes,
            30.0,
            -70.0,
        );
        assert_eq!(
            result,
            Some("Up"),
            "vertical-dominant swipe exiting pad must fire (no end.y constraint \
             on vertical direction)"
        );
    }

    /// The classic diagonal swipe-typing gesture: starts above the
    /// pad area (in keys) and curves slightly — sometimes the
    /// gesture's first sample lands AT the pad boundary. With a
    /// HORIZONTAL dominance, the original end.y guard is still
    /// active and must reject this so swipe-typing doesn't get
    /// hijacked.
    #[test]
    fn bottom_pad_horizontal_with_end_in_keys_rejected() {
        let boxes = fake_keys_with_bottom(100.0);
        // dx=80 dominant horizontal, dy=-30. End at y=80 — clearly
        // above max_key_bottom - 10 = 90. Should reject because
        // horizontal end-constraint says end.y must be > 90.
        let result = detect_bottom_pad_arrow(
            &pt(100.0, 110.0),
            &pt(180.0, 80.0),
            &boxes,
            80.0,
            -30.0,
        );
        assert_eq!(
            result,
            None,
            "horizontal swipe whose end exits pad upward must be rejected — \
             this is the diagonal swipe-typing protection"
        );
    }

    /// Swipe NOT starting in the pad returns None regardless of
    /// direction. The start-in-pad gate is the primary intent signal.
    #[test]
    fn swipe_starting_above_pad_returns_none() {
        let boxes = fake_keys_with_bottom(100.0);
        // Start in keys area (y=50, well above pad bottom at y=100).
        // Even with a clean Up swipe, the gesture shouldn't trigger
        // the pad cursor-nav.
        let result = detect_bottom_pad_arrow(
            &pt(100.0, 50.0),
            &pt(100.0, -10.0),
            &boxes,
            0.0,
            -60.0,
        );
        assert_eq!(result, None);
    }

    /// Below the minimum length, classify_arrow_swipe returns None,
    /// so detect_bottom_pad_arrow also returns None. Tap-jitter
    /// in the pad shouldn't move the cursor.
    #[test]
    fn bottom_pad_short_movement_no_fire() {
        let boxes = fake_keys_with_bottom(100.0);
        // 20px movement — below MIN_LEN_PX of 28.
        let result = detect_bottom_pad_arrow(
            &pt(100.0, 120.0),
            &pt(100.0, 140.0),
            &boxes,
            0.0,
            20.0,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn dispatch_mode_classification() {
        assert_eq!(LayoutStyle::Standard.dispatch_mode(), DispatchMode::Standard);
        assert_eq!(LayoutStyle::CardinalGrid.dispatch_mode(), DispatchMode::Cardinal);
        assert_eq!(LayoutStyle::Stenotype.dispatch_mode(), DispatchMode::Stenotype);
    }

    #[test]
    fn standard_mode_runs_freeform_swipe() {
        // Only Standard runs the freeform-swipe interceptors;
        // experimental modes suppress them.
        assert!(!DispatchMode::Standard.suppresses_freeform_swipe());
        assert!(DispatchMode::Cardinal.suppresses_freeform_swipe());
        assert!(DispatchMode::Stenotype.suppresses_freeform_swipe());
    }

    #[test]
    fn experimental_modes_use_cardinal_dispatch() {
        assert!(!DispatchMode::Standard.uses_cardinal_dispatch());
        assert!(DispatchMode::Cardinal.uses_cardinal_dispatch());
        assert!(DispatchMode::Stenotype.uses_cardinal_dispatch());
    }

    #[test]
    fn state_handler_layout_name_contract() {
        // The StateChanged handler classifies the current layout as a
        // "transient page" (symbols/numbers/emoji/arrows) vs a
        // "letter-family" layout to decide whether a shift toggle does
        // an in-place label update or a full restore-rebuild. That
        // logic keys off layout NAMES. If a layout is renamed without
        // updating the handler, shift handling silently breaks. This
        // pins the names so a rename fails loudly here.
        use crate::layouts::Layout;

        // Transient pages — must match the handler's is_transient_page set.
        assert_eq!(Layout::default_symbols().name, "symbols");
        assert_eq!(Layout::default_numbers().name, "numbers");
        assert_eq!(Layout::default_arrows().name, "arrows");

        // Letter-family — must NOT be in the transient set (so shift
        // updates labels in place rather than rebuilding).
        let transient = ["symbols", "numbers", "emoji", "arrows"];
        for layout in [
            Layout::default_qwerty(),
            Layout::default_dvorak_ortholinear(),
            Layout::default_cardinal_grid(),
            Layout::default_cardinal_grid_dvorak(),
        ] {
            assert!(
                !transient.contains(&layout.name.as_str()),
                "letter-family layout '{}' must not collide with a transient page name",
                layout.name
            );
        }
    }
}

#[cfg(test)]
#[path = "keyboard_behaviors_tests.rs"]
mod keyboard_behaviors_tests;
