//! In-keyboard settings panel.
//!
//! When the user taps the `⚙` settings key, the keyboard view swaps its
//! content for this panel. The panel exposes settings in three groups:
//!
//!   1. **Layout** — tap-to-toggle for Letters (QWERTY/Dvorak),
//!      Geometry (Staggered/Ortho), and Density (Normal/Compact).
//!   2. **Colors** — eight color pickers for the user-customizable
//!      palette (background, key fill, text, accents, etc.).
//!   3. **Experimental** — Cardinal grid mode, Voice input, Swipe trail.
//!
//! Changes are persisted to `~/.config/rusty-board/config.toml` on
//! every toggle so user choices survive restart without needing to hit
//! a Save button. Broadcast to the keyboard view immediately so the
//! visible layout switches without reopening the app.
//!
//! The panel is a free-standing widget; the `KeyboardView` swaps it in
//! and out as a child of its overlay. We don't pop up a separate window
//! because (a) on-mobile, secondary windows are awful, and (b) staying
//! inside our layer surface means the settings UI gets the same docking
//! and HiDPI treatment as the rest of the keyboard.

use crate::config::Config;
use crate::ui::tap_hold::{TapHoldButton, TapHoldConfig};
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tracing::{info, warn};

/// Callback fired on every toggle/change inside settings, while the
/// panel remains open. Used for live updates that should be reflected
/// in the keyboard rendering (e.g. CSS color changes, compact mode)
/// without closing the settings UI.
///
/// The caller must NOT tear down the settings panel from this callback
/// — that's the on-Done path's job.
pub type SettingsChangeCallback = Rc<dyn Fn(&Config)>;

/// Callback fired exactly once when the user taps Done. The caller is
/// expected to close the settings panel, rebuild the keyboard layout,
/// and (re)apply any deferred config changes.
pub type SettingsDoneCallback = Rc<dyn Fn(&Config)>;

/// Back-compat alias — the older callback shape used a single
/// fire-on-every-change handler. Some integration points may still
/// import this name; keep it as an alias for `SettingsChangeCallback`.
pub type SettingsAppliedCallback = SettingsChangeCallback;

pub struct SettingsPanel {
    widget: gtk4::Box,
}

impl SettingsPanel {
    /// Build a settings panel pre-populated from `initial_cfg`.
    ///
    /// `on_change` is invoked on every toggle — use for live CSS /
    /// theme updates that don't require closing the panel.
    ///
    /// `on_done` is invoked when the user taps Done — use to close the
    /// panel and rebuild the keyboard.
    pub fn new(
        initial_cfg: Config,
        on_change: SettingsChangeCallback,
        on_done: SettingsDoneCallback,
    ) -> Self {
        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(4)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(8)
            .margin_end(8)
            .css_classes(["settings-panel", "settings-compact"])
            .build();

        // Header bar with title and Done button.
        let header = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(8)
            .build();
        let title = gtk4::Label::builder()
            .label("Settings")
            .css_classes(["heading"])
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .build();
        let done_btn = gtk4::Button::builder()
            .label("Done")
            .css_classes(["suggested-action", "settings-done"])
            .build();
        header.append(&title);
        header.append(&done_btn);
        root.append(&header);

        // Scrolled container — settings are now substantial on phone.
        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .build();
        let scroll_inner = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(6)
            .halign(gtk4::Align::Center)
            // Width is intentionally content-driven. An earlier version
            // set width_request(380) thinking it would CAP the width on
            // wide screens, but width_request is a MINIMUM — on a phone
            // narrower than 380px (e.g. ~360px logical) it forced
            // horizontal overflow or clipping. The keyboard's
            // MAX_KEYBOARD_WIDTH cap on the outer keyboard_box already
            // prevents the panel from sprawling on tablet/desktop, so
            // settings naturally inherit that constraint and center.
            .build();
        scroll.set_child(Some(&scroll_inner));
        root.append(&scroll);

        // Shared mutable state — toggles read from / write to this.
        let cfg_state = Rc::new(RefCell::new(initial_cfg));

        // Helper: persist + notify-on-change. This fires on every
        // toggle. Crucially, it does NOT close the panel — the caller's
        // on_change handler should only update CSS/cache, never tear
        // down the settings UI. Tearing down would happen as part of
        // `on_done` instead.
        // Notify chain: settings -> persist config -> rebuild the
        // keyboard view. We DEBOUNCE this — every slider value
        // change calls notify(), but we don't want to disk-write
        // and rebuild the entire keyboard 100 times during a slider
        // drag. Schedule a 250ms timeout instead; subsequent
        // notify() calls within that window reset the timer.
        //
        // The atomic flags (width_scale, height_scale, etc) are
        // ALREADY applied live by their setters; notify() just
        // persists to disk and triggers a layout rebuild, both of
        // which are expensive.
        let pending_token: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let notify: Rc<dyn Fn()> = {
            let cfg_state = cfg_state.clone();
            let on_change = on_change.clone();
            let pending_token = pending_token.clone();
            Rc::new(move || {
                // Bump the generation counter; the timeout
                // callback checks this and bails if its own token
                // is stale (i.e. a later notify() came in).
                let my_token = pending_token.get().wrapping_add(1);
                pending_token.set(my_token);
                let cfg_state = cfg_state.clone();
                let on_change = on_change.clone();
                let pending_token = pending_token.clone();
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(250),
                    move || {
                        if pending_token.get() != my_token {
                            // A newer notify() came in; let it
                            // handle the work.
                            return;
                        }
                        let cfg = cfg_state.borrow();
                        if let Err(e) = cfg.save() {
                            warn!("settings: failed to persist config: {}", e);
                        }
                        #[cfg(feature = "llm")]
                        crate::ui::window::set_live_llm_config(cfg.llm.clone());
                        on_change(&*cfg);
                    },
                );
            })
        };

        // ================================================================
        // Section: Layout
        // ================================================================
        scroll_inner.append(&section_header("Layout"));

        // Letters — QWERTY ↔ Dvorak. Base letter system only;
        // experimental modes (cardinal, stenotype) are a separate
        // picker in the Experimental section.
        scroll_inner.append(&build_setting_row(
            "Letters",
            "Base letter layout. QWERTY or Dvorak.",
            &["QWERTY", "Dvorak"],
            letters_index(&cfg_state.borrow()),
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    cfg_state.borrow_mut().layout.letters = if idx == 1 {
                        "dvorak".into()
                    } else {
                        "qwerty".into()
                    };
                    info!(
                        "settings: letters -> {}",
                        cfg_state.borrow().layout.letters
                    );
                    notify();
                }
            },
        ));

        // Geometry — Staggered ↔ Ortholinear.
        scroll_inner.append(&build_setting_row(
            "Geometry",
            "Staggered matches a physical keyboard; ortholinear is a strict grid.",
            &["Staggered", "Ortholinear"],
            row_style_index(&cfg_state.borrow()),
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    cfg_state.borrow_mut().layout.row_style = if idx == 1 {
                        "ortholinear".into()
                    } else {
                        "staggered".into()
                    };
                    info!(
                        "settings: row_style -> {}",
                        cfg_state.borrow().layout.row_style
                    );
                    notify();
                }
            },
        ));

        // Density — Normal vs Compact.
        scroll_inner.append(&build_setting_row(
            "Density",
            "Compact reduces vertical padding for one-handed reach.",
            &["Normal", "Compact"],
            if cfg_state.borrow().layout.compact { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    cfg_state.borrow_mut().layout.compact = idx == 1;
                    info!(
                        "settings: compact -> {}",
                        cfg_state.borrow().layout.compact
                    );
                    notify();
                }
            },
        ));

        // Single-hand mode — compacts the layout into the user's thumb
        // arc. Off = full-width (default). Right = compaction biased
        // toward right thumb (anchor bottom-right). Left = mirror.
        //
        // First-launch behavior: when the user picks Right or Left for
        // the first time, we synthesize a default arc using the screen
        // width. Later we'll add explicit calibration UI; for now the
        // default is sensible enough to use.
        let current_hand_idx = match cfg_state.borrow().layout.thumb_arc.as_ref() {
            None => 0,
            Some(arc) => match arc.anchor {
                crate::config::ThumbAnchor::BottomRight => 1,
                crate::config::ThumbAnchor::BottomLeft => 2,
            },
        };
        scroll_inner.append(&build_setting_row(
            "Single-hand mode",
            "Compacts the keyboard into your thumb's reachable arc.",
            &["Off", "Right", "Left"],
            current_hand_idx,
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let new_arc = match idx {
                        0 => None,
                        1 => Some(default_arc_for_screen(
                            crate::config::ThumbAnchor::BottomRight,
                        )),
                        2 => Some(default_arc_for_screen(
                            crate::config::ThumbAnchor::BottomLeft,
                        )),
                        _ => None,
                    };
                    cfg_state.borrow_mut().layout.thumb_arc = new_arc;
                    info!(
                        "settings: single-hand mode -> {}",
                        match idx {
                            1 => "Right",
                            2 => "Left",
                            _ => "Off",
                        }
                    );
                    notify();
                }
            },
        ));

        // ================================================================
        // DEFERRED SECTIONS
        // ================================================================
        // Everything below the Layout section (Colors, Experimental,
        // LLM, Maintenance) is built on an idle callback AFTER the
        // panel is shown, rather than synchronously here. On the FP5
        // the ~30 setting-rows (each ~5-6 widgets) cost a visible
        // beat to construct + first-allocate; deferring the
        // below-the-fold sections lets the panel appear instantly
        // with the Layout controls, then fill in within a frame or
        // two. The user almost never needs Maintenance/LLM in the
        // first 100ms of opening settings.
        //
        // All captured state is Rc/clone-friendly. The closure runs
        // exactly once (idle_add_local_once).
        let build_deferred = {
            let scroll_inner = scroll_inner.clone();
            let cfg_state = cfg_state.clone();
            let notify = notify.clone();
            move || {
        // ================================================================
        // Section: Colors
        // ================================================================
        scroll_inner.append(&section_header("Colors"));
        scroll_inner.append(&build_color_section(cfg_state.clone(), notify.clone()));

        // ================================================================
        // Section: Experimental
        // ================================================================
        scroll_inner.append(&section_header("Experimental"));

        // Experimental keyboard mode — None / Cardinal / Stenotype.
        // Only available when the `input-modes` feature is compiled
        // in (full-keyboard profile and up). Basic builds skip this
        // entirely so the user can't select a surface the build
        // doesn't support.
        #[cfg(feature = "input-modes")]
        scroll_inner.append(&build_setting_row(
            "Keyboard mode",
            "Off = normal keyboard. Cardinal = 3×3 swipe grid. Stenotype = 4-button steno (very experimental). Words = 5-col cardinal with prose / C / Rust / Python clusters (cycle via the lang button in the bottom row).",
            &["Off", "Cardinal", "Stenotype", "Words"],
            experimental_mode_index(&cfg_state.borrow()),
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let mode = match idx {
                        1 => "cardinal",
                        2 => "stenotype",
                        3 => "cardinal_words",
                        _ => "none",
                    };
                    {
                        let mut c = cfg_state.borrow_mut();
                        c.layout.experimental_mode = mode.into();
                        // Keep the deprecated bool in sync for any
                        // code still reading it.
                        c.layout.cardinal_enabled = mode == "cardinal";
                    }
                    info!("settings: experimental_mode -> {}", mode);
                    notify();
                }
            },
        ));

        // Voice input.
        scroll_inner.append(&build_setting_row(
            "Voice input",
            "Show 🎤 key. Requires a Whisper model.",
            &["Off", "On"],
            if cfg_state.borrow().voice.enabled { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let enabled = idx == 1;
                    cfg_state.borrow_mut().voice.enabled = enabled;
                    crate::ui::window::set_voice_key_visible(enabled);
                    notify();
                }
            },
        ));

        // Streaming partial transcription. Runs inference every ~600ms
        // during recording to show a live partial in the microterm.
        // CPU-heavy on phone — off may be preferable if the device
        // overheats or you only want the final result. The final
        // transcription (on tap-to-stop) is unaffected by this toggle.
        scroll_inner.append(&build_setting_row(
            "Live transcription",
            "Show partial text while recording. Higher CPU usage.",
            &["Off", "On"],
            if cfg_state.borrow().voice.streaming { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let enabled = idx == 1;
                    cfg_state.borrow_mut().voice.streaming = enabled;
                    crate::ui::window::set_voice_streaming_enabled(enabled);
                    notify();
                }
            },
        ));

        // Whisper language hint. "auto" lets whisper detect; the
        // others bias decoding toward the selected language and
        // are typically 10-30% faster on phone-grade hardware.
        let lang_options = ["auto", "en", "fr", "es", "de"];
        let current_lang = cfg_state.borrow().voice.language.clone();
        let lang_idx = lang_options
            .iter()
            .position(|c| *c == current_lang)
            .unwrap_or(0);
        scroll_inner.append(&build_setting_row(
            "Whisper language",
            "Bias decoding. \"auto\" is slowest. Restart to take effect.",
            &lang_options,
            lang_idx,
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                let opts = lang_options.iter().map(|s| s.to_string()).collect::<Vec<_>>();
                move |idx| {
                    if let Some(code) = opts.get(idx) {
                        cfg_state.borrow_mut().voice.language = code.clone();
                        notify();
                    }
                }
            },
        ));

        // Voice transcript log. Privacy-sensitive — off by default.
        // When on, every transcription is written to
        // ~/.local/share/rusty-board/voice-log.jsonl. The /history
        // slash command reads this file.
        scroll_inner.append(&build_setting_row(
            "Voice transcript log",
            "Save voice text to ~/.local/share/rusty-board/voice-log.jsonl. Off for privacy.",
            &["Off", "On"],
            if cfg_state.borrow().voice.log_transcripts { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let enabled = idx == 1;
                    cfg_state.borrow_mut().voice.log_transcripts = enabled;
                    crate::ui::window::set_voice_log_enabled(enabled);
                    notify();
                }
            },
        ));

        // Voice-type mode: shrinks the keyboard to ~50% of its
        // normal height to give more room for the microterm and
        // wrapped-context display. Useful when dictating long
        // text, code, or want to see more of what you're editing.
        // Not stored in config — it's a session toggle.
        scroll_inner.append(&build_setting_row(
            "Voice-type mode",
            "Shrink the keys to half height to see more text. Good for dictating code blocks.",
            &["Off", "On"],
            if crate::ui::keyboard_view::voice_type_mode() { 1 } else { 0 },
            {
                let notify = notify.clone();
                move |idx| {
                    crate::ui::keyboard_view::set_voice_type_mode(idx == 1);
                    notify();
                }
            },
        ));

        // Character drawing mode: ink Greek letters, math
        // symbols, etc. on the spacebar. Multi-stroke; tap-and-
        // hold the spacebar to recognize and commit. The path
        // database is built-in and covers Greek lowercase +
        // common math operators.
        scroll_inner.append(&build_setting_row(
            "Character drawing",
            "Draw Greek/math symbols on the spacebar. Tap-and-hold space to recognize.",
            &["Off", "On"],
            if crate::keyboard::character_draw::draw_mode_enabled() { 1 } else { 0 },
            {
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    crate::keyboard::character_draw::set_draw_mode(on);
                    crate::ui::window::post_microterm_status(if on {
                        "Drawing mode ON — draw on spacebar, long-press to commit"
                    } else {
                        "Drawing mode OFF"
                    });
                    notify();
                }
            },
        ));

        // Auto-capitalize. Off by default — many users find it
        // intrusive, especially when typing names, code, or
        // anything where the first letter should NOT be capitalized.
        scroll_inner.append(&build_setting_row(
            "Auto-capitalize",
            "Capitalize the first letter after a period or at start of input.",
            &["Off", "On"],
            if cfg_state.borrow().behavior.auto_capitalize { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().behavior.auto_capitalize = on;
                    crate::keyboard::state::set_auto_cap_enabled(on);
                    notify();
                }
            },
        ));

        // Hairpin / "swipe-back-to-capitalize" gesture in cardinal
        // mode. Some users find this fires accidentally when they
        // course-correct mid-swipe. This toggle disables the
        // hairpin classifier; tapping shift still works.
        scroll_inner.append(&build_setting_row(
            "Swipe-back capitalize",
            "Cardinal: swipe out and back to capitalize. Disable if too sensitive.",
            &["Off", "On"],
            if cfg_state.borrow().gestures.shift_swipe { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().gestures.shift_swipe = on;
                    crate::gestures::cardinal::set_shift_swipe_enabled(on);
                    notify();
                }
            },
        ));

        // Lenient swipe recognition. When on, the decoder accepts
        // wider edit distances, less spatial accuracy, and weights
        // frequency more heavily. Good for fast, sloppy, or
        // circular-ish swipes; bad for distinguishing very similar
        // words. Off by default.
        scroll_inner.append(&build_setting_row(
            "Lenient swipe",
            "Accept faster, sloppier swipes. Trades precision for forgiveness.",
            &["Off", "On"],
            if cfg_state.borrow().gestures.lenient_swipe { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().gestures.lenient_swipe = on;
                    crate::gestures::recognizer::set_lenient_swipe_enabled(on);
                    crate::gestures::cardinal::set_cardinal_lenient(on);
                    notify();
                }
            },
        ));

        // AI swipe assist — master toggle for the personalization
        // signals (adaptive dictionary that learns user's vocabulary,
        // n-gram next-word model that disambiguates ambiguous swipes
        // via context). When off, the recognizer uses only the
        // shape-based dictionary matching with no personalization.
        // Defaults on; users can opt out here.
        scroll_inner.append(&build_setting_row(
            "AI swipe assist",
            "Learn your vocabulary & disambiguate ambiguous swipes via context. Off uses the plain shape recognizer only.",
            &["Off", "On"],
            if cfg_state.borrow().gestures.ai_swipe_assist { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().gestures.ai_swipe_assist = on;
                    crate::gestures::ai_swipe::set_ai_swipe_assist_enabled(on);
                    notify();
                }
            },
        ));

        // Cardinal diagonals — enable 8-way swipes (NE/SE/SW/NW) so
        // each cardinal-grid key carries extra characters on its
        // diagonals. Off = diagonals fold to the nearest of N/E/S/W
        // (more forgiving). Only meaningful in cardinal mode, so
        // gated with the rest of the experimental input surfaces.
        #[cfg(feature = "input-modes")]
        scroll_inner.append(&build_setting_row(
            "Cardinal diagonals",
            "8-way swipes: diagonal flicks (↗ ↘ ↙ ↖) reach extra keys. Off folds them to N/E/S/W.",
            &["Off", "On"],
            if cfg_state.borrow().gestures.cardinal_diagonals { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().gestures.cardinal_diagonals = on;
                    crate::gestures::cardinal::set_cardinal_diagonals(on);
                    notify();
                }
            },
        ));

        // Swipe speed mode — tunes how readily a slowdown counts as a
        // deliberate letter-marking stop. Slow swipers pause gently
        // (raise the threshold); fast swipers only briefly decelerate
        // (lower it). Applies to swipe-typing segmentation.
        {
            let initial = match cfg_state.borrow().gestures.swipe_speed_mode.as_str() {
                "slow" => 0,
                "fast" => 2,
                _ => 1,
            };
            scroll_inner.append(&build_setting_row(
                "Swipe speed",
                "How hard you pause to mark a letter. Slow = gentle pauses count; Fast = only near-stops.",
                &["Slow", "Normal", "Fast"],
                initial,
                {
                    let cfg_state = cfg_state.clone();
                    let notify = notify.clone();
                    move |idx| {
                        let mode = match idx {
                            0 => "slow",
                            2 => "fast",
                            _ => "normal",
                        };
                        cfg_state.borrow_mut().gestures.swipe_speed_mode = mode.to_string();
                        crate::gestures::swipe::set_swipe_speed_mode(mode);
                        notify();
                    }
                },
            ));
        }

        // Auto-space between swipes — insert a space automatically
        // when one swipe-typed word follows another, so you can swipe
        // word after word without tapping space.
        scroll_inner.append(&build_setting_row(
            "Auto-space swipes",
            "Add a space automatically between consecutive swipe-typed words.",
            &["Off", "On"],
            if cfg_state.borrow().gestures.swipe_auto_space { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().gestures.swipe_auto_space = on;
                    crate::gestures::swipe::set_swipe_auto_space(on);
                    notify();
                }
            },
        ));

        // Glass effect — faux translucency + per-row refraction
        // gradient. Doesn't capture actual screen content (the
        // Wayland security model forbids that without compositor
        // cooperation), but applies a top-to-bottom material-flat-
        // glass-style gradient on top of the standard semi-
        // transparent keyboard so the wallpaper/app behind shows
        // through with visual interest. Default off — solid is
        // more legible for typing.
        scroll_inner.append(&build_setting_row(
            "Glass effect",
            "Translucent keyboard with refraction gradient. Wallpaper shows through.",
            &["Off", "On"],
            if cfg_state.borrow().appearance.glass_effect { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().appearance.glass_effect = on;
                    crate::ui::window::set_glass_effect(on);
                    notify();
                }
            },
        ));

        // Focus-tray mode: when a text field gets focus, show a
        // tiny arrow-keys tray instead of the full keyboard. User
        // taps "ABC" in the tray to summon the full keyboard.
        // Applies on the NEXT focus-in after toggling — not the
        // current focus state.
        scroll_inner.append(&build_setting_row(
            "Focus-tray mode",
            "On focus, show arrows-only tray; tap ABC to summon full keyboard.",
            &["Off", "On"],
            if cfg_state.borrow().behavior.focus_show_tray { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().behavior.focus_show_tray = on;
                    crate::ui::window::set_focus_show_tray(on);
                    notify();
                }
            },
        ));

        // Auto-hide on no focus: when off, the keyboard stays
        // visible even when no text field is focused. Useful for
        // debugging or always-on usage. Default on.
        scroll_inner.append(&build_setting_row(
            "Auto-hide on no focus",
            "Hide the keyboard when no text field has focus.",
            &["Off", "On"],
            if cfg_state.borrow().behavior.auto_hide_on_no_focus.unwrap_or(true) { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().behavior.auto_hide_on_no_focus = Some(on);
                    crate::ui::window::set_auto_hide_enabled(on);
                    notify();
                }
            },
        ));

        // Number row: prepend a row of digits (1234567890) above
        // the letters. Adds a full row of height; nice for those
        // who type a lot of mixed text + numbers.
        scroll_inner.append(&build_setting_row(
            "Number row",
            "Show a digit row above the letters.",
            &["Off", "On"],
            if cfg_state.borrow().layout.number_row { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    cfg_state.borrow_mut().layout.number_row = idx == 1;
                    notify();
                }
            },
        ));

        // ---- LLM settings ----
        scroll_inner.append(&section_header("LLM"));
        scroll_inner.append(&build_setting_row(
            "LLM assistant",
            "Enable the ✨ key. Sends prompts to an OpenAI-compatible API \
             (ollama, llama.cpp, etc).",
            &["Off", "On"],
            if cfg_state.borrow().llm.enabled { 1 } else { 0 },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let enabled = idx == 1;
                    cfg_state.borrow_mut().llm.enabled = enabled;
                    crate::ui::window::set_llm_key_visible(enabled);
                    notify();
                }
            },
        ));

        scroll_inner.append(&build_text_row(
            "LLM endpoint",
            "URL of an OpenAI-compatible chat completions API.",
            &cfg_state.borrow().llm.endpoint,
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |text| {
                    cfg_state.borrow_mut().llm.endpoint = text;
                    notify();
                }
            },
        ));

        // LLM model picker: dropdown polled from the configured
        // endpoint at panel open. Initially seeded with the saved
        // model + a few common defaults so the user always sees
        // SOMETHING tappable; the async fetch fills in the actual
        // installed models when it returns.
        //
        // Why a dropdown and not a text entry: the keyboard-in-
        // keyboard nested-input case doesn't work (we'd be typing
        // INTO ourselves, which causes focus/IM loops). A
        // dropdown avoids the issue entirely — user just picks.
        {
            let current_model = cfg_state.borrow().llm.model.clone();
            let endpoint = cfg_state.borrow().llm.endpoint.clone();
            let model_row = build_dropdown_row(
                "LLM model",
                "Model name. Polled from your Ollama endpoint.",
                build_initial_model_list(&current_model),
                &current_model,
                {
                    let cfg_state = cfg_state.clone();
                    let notify = notify.clone();
                    move |selected: String| {
                        if selected.is_empty() {
                            return;
                        }
                        let mut c = cfg_state.borrow_mut();
                        if c.llm.model != selected {
                            c.llm.model = selected;
                            drop(c);
                            notify();
                        }
                    }
                },
            );
            scroll_inner.append(&model_row.widget);
            // Kick off async fetch. The dropdown will update if
            // the endpoint responds with a model list.
            spawn_ollama_model_fetch(endpoint, current_model, model_row.update_fn);
        }

        scroll_inner.append(&build_text_row(
            "LLM API key",
            "Bearer token (leave blank for local servers).",
            &cfg_state.borrow().llm.api_key,
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |text| {
                    cfg_state.borrow_mut().llm.api_key = text;
                    notify();
                }
            },
        ));

        // Key repeat — when off, holding a key emits exactly one event.
        // Useful for people who often hold keys too long unintentionally
        // (e.g. on a phone where finger pressure varies).
        //
        // NOTE: as of this build, repeat-on-hold isn't actually wired
        // into the engine yet. The toggle persists so it's there when
        // we implement it, but right now every key emits once per
        // press regardless of this setting.
        scroll_inner.append(&build_setting_row(
            "Key repeat",
            "When on, holding a key repeats it (currently unimplemented; off is the de-facto behavior).",
            &["Off", "On"],
            if cfg_state.borrow().behavior.key_repeat {
                1
            } else {
                0
            },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    cfg_state.borrow_mut().behavior.key_repeat = idx == 1;
                    notify();
                }
            },
        ));

        // Swipe typing — full word recognition on drag across letters.
        // When OFF, the recognizer pipeline is skipped: drags from the
        // spacebar become a pure arrow-pad (Left/Right/Up/Down via the
        // existing arrow-swipe detection, which runs regardless), and
        // drags from letter keys become tap-or-cancel. Useful for users
        // who want predictable tap typing + spacebar cursor navigation
        // without the chance of an accidental word commit.
        scroll_inner.append(&build_setting_row(
            "Swipe typing",
            "Drag across letters to type whole words. Off → spacebar drag becomes arrow keys.",
            &["Off", "On"],
            if cfg_state.borrow().gestures.swipe_typing {
                1
            } else {
                0
            },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().gestures.swipe_typing = on;
                    crate::gestures::swipe::set_swipe_typing_enabled(on);
                    notify();
                }
            },
        ));

        // Debug trail styling — visual aid. Renders the swipe trail
        // differently per gesture category so it's obvious on-device
        // which dispatch branch a gesture takes: dotted = cardinal,
        // striped = swipe-word, desaturated = normal/ignored. Off by
        // default; this is a debugging tool, not a daily-use feature.
        scroll_inner.append(&build_setting_row(
            "Debug trail styling",
            "Color/dash the swipe trail by gesture type (dotted=cardinal, striped=word, faded=ignored). For visual debugging.",
            &["Off", "On"],
            if cfg_state.borrow().appearance.debug_trail_styling {
                1
            } else {
                0
            },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    let on = idx == 1;
                    cfg_state.borrow_mut().appearance.debug_trail_styling = on;
                    crate::ui::swipe_trail::set_trail_debug_styling(on);
                    notify();
                }
            },
        ));


        // for the future word-prediction work.
        //
        // NOTE: changing this requires a keyboard rebuild (window.rs
        // adds the bar conditionally at construction). The on_done
        // callback rebuilds via rebuild_view, which is good enough —
        // user just has to hit Done to see the change.
        scroll_inner.append(&build_setting_row(
            "Suggestion bar",
            "Reserve a top strip for future word suggestions (currently empty).",
            &["Off", "On"],
            if cfg_state.borrow().appearance.show_suggestions {
                1
            } else {
                0
            },
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |idx| {
                    cfg_state.borrow_mut().appearance.show_suggestions = idx == 1;
                    notify();
                }
            },
        ));

        // ---- Section: Maintenance ----
        //
        // Actions in this section use TapHoldButton with the QML
        // ToolBtn pattern: tap = light/safe action (show info), hold
        // = the destructive or expensive action. This matches the
        // voiceterm UX and gives the user a visible commitment
        // (progress bar) before anything dangerous happens.
        scroll_inner.append(&section_header("Maintenance"));

        // Width adjustment slider: ±50% (50-150%). Live-applies to
        // the keyboard via set_width_scale; notify rebuilds the
        // layout so the new size takes immediate effect. Widened
        // from ±25% on user request.
        scroll_inner.append(&build_slider_row(
            "Key width",
            "Adjust how wide each key is. Useful for finger size.",
            50.0,
            150.0,
            (cfg_state.borrow().layout.width_scale * 100.0) as f64,
            "%",
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |v_pct| {
                    let scale = (v_pct / 100.0) as f32;
                    cfg_state.borrow_mut().layout.width_scale = scale;
                    crate::ui::keyboard_view::set_width_scale(scale);
                    notify();
                }
            },
        ));

        // Height adjustment slider: ±50% (50-150%).
        scroll_inner.append(&build_slider_row(
            "Key height",
            "Adjust how tall the keyboard is.",
            50.0,
            150.0,
            (cfg_state.borrow().layout.height_scale * 100.0) as f64,
            "%",
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |v_pct| {
                    let scale = (v_pct / 100.0) as f32;
                    cfg_state.borrow_mut().layout.height_scale = scale;
                    crate::ui::keyboard_view::set_height_scale(scale);
                    notify();
                }
            },
        ));

        // Bottom padding slider: 0-240px (doubled from 0-120). For
        // users who hold the phone low or want more room for the
        // bottom-pad horizontal-swipe cursor-nav feature.
        scroll_inner.append(&build_slider_row(
            "Bottom padding",
            "Reserve space below the keyboard. Useful if your hands sit low on the phone, or if you want room for the bottom-pad cursor-swipe gesture.",
            0.0,
            240.0,
            cfg_state.borrow().layout.bottom_pad_px as f64,
            "px",
            {
                let cfg_state = cfg_state.clone();
                let notify = notify.clone();
                move |v_px| {
                    let px = v_px.round() as i32;
                    cfg_state.borrow_mut().layout.bottom_pad_px = px;
                    crate::ui::keyboard_view::set_bottom_pad(px);
                    notify();
                }
            },
        ));

        // Reset config to defaults. Tap shows a brief explanation;
        // hold actually wipes config and rebuilds with defaults.
        {
            let cfg = TapHoldConfig {
                hold_interval_ms: 1200, // longer hold for destructive action
                release_snap_ms: 120,
                show_progress: true,
            };
            let btn = TapHoldButton::new("Reset to defaults", cfg);
            btn.add_css_class("danger");
            let cfg_state_for_hold = cfg_state.clone();
            let notify_for_hold = notify.clone();
            btn.on_tap(|| {
                info!("Reset-to-defaults: tap (hold to confirm)");
            });
            btn.on_hold(move || {
                info!("Reset-to-defaults: held — clearing config");
                *cfg_state_for_hold.borrow_mut() = Config::default();
                notify_for_hold();
            });
            scroll_inner.append(btn.widget());
        }

        // Test LLM endpoint. Tap = quick HEAD-style check (we just
        // verify the URL parses + the host resolves). Hold = full
        // chat completion with a canned prompt; reports the
        // round-trip latency.
        {
            let cfg = TapHoldConfig::default();
            let btn = TapHoldButton::new("Test LLM endpoint", cfg);
            let cfg_state_for_tap = cfg_state.clone();
            btn.on_tap(move || {
                let endpoint = cfg_state_for_tap.borrow().llm.endpoint.clone();
                info!("LLM endpoint test (tap): {}", endpoint);
            });
            let cfg_state_for_hold = cfg_state.clone();
            btn.on_hold(move || {
                let llm_cfg = cfg_state_for_hold.borrow().llm.clone();
                info!("LLM endpoint test (hold): full chat to {}", llm_cfg.endpoint);
                // The actual test runs in a worker thread; we just
                // log the result. A future patch can surface it to
                // the microterm.
                std::thread::Builder::new()
                    .name("llm-test".into())
                    .spawn(move || {
                        #[cfg(feature = "llm")]
                        {
                            use crate::llm::client::{ClientConfig, LlmClient};
                            crate::ui::window::post_microterm_status(
                                "LLM test: connecting...",
                            );
                            let client_cfg = ClientConfig {
                                endpoint: llm_cfg.endpoint.clone(),
                                model: llm_cfg.model.clone(),
                                api_key: llm_cfg.api_key,
                                system_prompt: llm_cfg.system_prompt,
                                timeout_ms: llm_cfg.timeout_ms,
                                max_tokens: 16,
                                temperature: 0.1,
                            };
                            let started = std::time::Instant::now();
                            match LlmClient::new(client_cfg)
                                .and_then(|c| c.complete("Say OK in one word."))
                            {
                                Ok(text) => {
                                    let ms = started.elapsed().as_millis();
                                    info!("LLM test ok ({}ms): {:?}", ms, text);
                                    crate::ui::window::post_microterm_status(
                                        format!(
                                            "LLM ok ({}ms): {}",
                                            ms,
                                            text.chars().take(40).collect::<String>()
                                        ),
                                    );
                                }
                                Err(e) => {
                                    warn!("LLM test failed: {}", e);
                                    let s = e.to_string();
                                    let trimmed: String =
                                        s.chars().take(60).collect();
                                    crate::ui::window::post_microterm_status(
                                        format!("LLM fail: {}", trimmed),
                                    );
                                }
                            }
                        }
                        #[cfg(not(feature = "llm"))]
                        {
                            warn!("LLM test: feature not compiled in");
                        }
                    })
                    .ok();
            });
            scroll_inner.append(btn.widget());
        }

        // Show LLM prompt history. Tap → most recent 3 prompts;
        // hold → up to 10. The microterm is the display surface
        // since the settings panel scroll view doesn't have room
        // for an embedded list.
        {
            let btn = TapHoldButton::new("Show prompt history", TapHoldConfig::default());
            btn.on_tap(|| {
                #[cfg(feature = "llm")]
                {
                    let entries = crate::ui::window::recent_llm_prompts(3);
                    if entries.is_empty() {
                        crate::ui::window::post_microterm_status(
                            "No LLM prompts yet.",
                        );
                    } else {
                        let joined = entries
                            .iter()
                            .enumerate()
                            .map(|(i, t)| {
                                format!(
                                    "{}: {}",
                                    i + 1,
                                    t.chars().take(40).collect::<String>()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" | ");
                        crate::ui::window::post_microterm_status(joined);
                    }
                }
            });
            btn.on_hold(|| {
                #[cfg(feature = "llm")]
                {
                    let entries = crate::ui::window::recent_llm_prompts(10);
                    if entries.is_empty() {
                        crate::ui::window::post_microterm_status(
                            "No LLM prompts yet.",
                        );
                    } else {
                        let joined = entries
                            .iter()
                            .enumerate()
                            .map(|(i, t)| {
                                format!(
                                    "{}: {}",
                                    i + 1,
                                    t.chars().take(60).collect::<String>()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("  ·  ");
                        crate::ui::window::post_microterm_status(joined);
                    }
                }
            });
            scroll_inner.append(btn.widget());
        }

            } // end build_deferred closure
        };

        // Schedule the deferred sections to build on the next idle
        // tick, after the panel has been returned and shown. This is
        // what makes settings open feel instant — the header + Layout
        // section are already in `scroll_inner`; the rest streams in.
        glib::idle_add_local_once(build_deferred);

        // ---- Done: persist once more and hand off to on_done. The
        //      caller's on_done is responsible for closing the panel
        //      and rebuilding the keyboard view. ----------------------
        let cb_done = on_done.clone();
        let cfg_done = cfg_state.clone();
        done_btn.connect_clicked(move |_| {
            let cfg = cfg_done.borrow();
            if let Err(e) = cfg.save() {
                warn!("settings: failed to persist config: {}", e);
            } else {
                info!("settings: persisted to {:?}", Config::config_path());
            }
            cb_done(&*cfg);
        });

        Self { widget: root }
    }

    pub fn widget(&self) -> &gtk4::Box {
        &self.widget
    }
}

// ---- Helpers --------------------------------------------------------

/// A section header label.
fn section_header(text: &str) -> gtk4::Label {
    gtk4::Label::builder()
        .label(text)
        .css_classes(["section-header"])
        .halign(gtk4::Align::Start)
        .margin_top(4)
        .build()
}

/// Build one labeled row: title, description, and a row of toggle
/// buttons. Currently selected button has `.suggested-action` style.
fn build_setting_row<F>(
    title: &str,
    subtitle: &str,
    options: &[&str],
    current_idx: usize,
    on_change: F,
) -> gtk4::Box
where
    F: Fn(usize) + 'static,
{
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(3)
        .build();

    let title_label = gtk4::Label::builder()
        .label(title)
        .css_classes(["heading"])
        .halign(gtk4::Align::Start)
        .build();
    let sub_label = gtk4::Label::builder()
        .label(subtitle)
        .css_classes(["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    row.append(&title_label);
    row.append(&sub_label);

    let button_row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .margin_top(3)
        .build();

    let buttons: Rc<Vec<gtk4::Button>> = Rc::new(
        options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                let mut classes = vec!["pill"];
                if i == current_idx {
                    classes.push("suggested-action");
                }
                gtk4::Button::builder()
                    .label(*opt)
                    .css_classes(classes)
                    .hexpand(true)
                    .build()
            })
            .collect(),
    );

    let on_change = Rc::new(on_change);
    for (i, btn) in buttons.iter().enumerate() {
        let buttons_ref = buttons.clone();
        let on_change = on_change.clone();
        btn.connect_clicked(move |_| {
            for (j, b) in buttons_ref.iter().enumerate() {
                if j == i {
                    b.add_css_class("suggested-action");
                } else {
                    b.remove_css_class("suggested-action");
                }
            }
            on_change(i);
        });
        button_row.append(btn);
    }

    row.append(&button_row);
    row
}

/// Build a settings row with a horizontal Scale slider. The
/// callback fires whenever the slider value changes; pass a unit
/// suffix ("%", "px", "ms") that's shown next to the live value.
///
/// We debounce-by-magnitude: the callback fires only when the
/// integer-rounded value crosses a step boundary, so dragging the
/// slider doesn't trigger 100 notifies/sec.
fn build_slider_row<F>(
    title: &str,
    subtitle: &str,
    min: f64,
    max: f64,
    initial: f64,
    unit: &str,
    on_change: F,
) -> gtk4::Box
where
    F: Fn(f64) + 'static,
{
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(2)
        .margin_top(4)
        .build();

    let header = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(4)
        .build();
    let title_label = gtk4::Label::builder()
        .label(title)
        .css_classes(["heading"])
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    let value_label = gtk4::Label::builder()
        .label(format!("{}{}", initial.round() as i32, unit))
        .css_classes(["caption", "dim-label"])
        .halign(gtk4::Align::End)
        .build();
    header.append(&title_label);
    header.append(&value_label);
    row.append(&header);

    let sub_label = gtk4::Label::builder()
        .label(subtitle)
        .css_classes(["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    row.append(&sub_label);

    let adj = gtk4::Adjustment::new(initial, min, max, 1.0, 5.0, 0.0);
    let scale = gtk4::Scale::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .adjustment(&adj)
        .hexpand(true)
        .draw_value(false)
        .build();
    let last_int = Rc::new(std::cell::Cell::new(initial.round() as i32));
    let on_change = Rc::new(on_change);
    let unit_owned: String = unit.to_string();
    {
        let value_label = value_label.clone();
        let last_int = last_int.clone();
        let on_change = on_change.clone();
        scale.connect_value_changed(move |s| {
            let v = s.value();
            let v_int = v.round() as i32;
            if v_int != last_int.get() {
                last_int.set(v_int);
                value_label.set_label(&format!("{}{}", v_int, unit_owned));
                on_change(v_int as f64);
            }
        });
    }
    row.append(&scale);
    row
}

/// Build a settings row with a text entry instead of pill buttons.
/// `on_change` fires whenever the user finishes editing — either on
/// Enter or when the entry loses focus. Used for free-form config
/// values like the LLM endpoint URL or model name.
fn build_text_row<F>(
    title: &str,
    subtitle: &str,
    initial: &str,
    on_change: F,
) -> gtk4::Box
where
    F: Fn(String) + 'static,
{
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(3)
        .build();

    let title_label = gtk4::Label::builder()
        .label(title)
        .css_classes(["heading"])
        .halign(gtk4::Align::Start)
        .build();
    let sub_label = gtk4::Label::builder()
        .label(subtitle)
        .css_classes(["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    row.append(&title_label);
    row.append(&sub_label);

    let entry = gtk4::Entry::builder()
        .text(initial)
        .hexpand(true)
        .margin_top(3)
        .build();

    let on_change = Rc::new(on_change);
    // Enter commits.
    {
        let on_change = on_change.clone();
        entry.connect_activate(move |e| {
            on_change(e.text().to_string());
        });
    }
    // Focus-leave also commits, so a user who taps elsewhere
    // doesn't lose their edit.
    {
        let on_change = on_change.clone();
        let focus = gtk4::EventControllerFocus::new();
        let entry_clone = entry.clone();
        focus.connect_leave(move |_| {
            on_change(entry_clone.text().to_string());
        });
        entry.add_controller(focus);
    }

    row.append(&entry);
    row
}

/// Returned by `build_dropdown_row` so callers can both append the
/// widget AND push more items into the dropdown later (e.g. after
/// an async fetch returns).
struct DropdownRow {
    widget: gtk4::Box,
    /// Updates the dropdown's item list. Pass a vec of model names;
    /// the function deduplicates against existing items and selects
    /// `desired_selection` if present, otherwise preserves the
    /// current selection.
    update_fn: Rc<dyn Fn(Vec<String>, String)>,
}

fn build_dropdown_row<F>(
    title: &str,
    subtitle: &str,
    initial_items: Vec<String>,
    initial_selection: &str,
    on_change: F,
) -> DropdownRow
where
    F: Fn(String) + 'static,
{
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(3)
        .build();

    let title_label = gtk4::Label::builder()
        .label(title)
        .css_classes(["heading"])
        .halign(gtk4::Align::Start)
        .build();
    let sub_label = gtk4::Label::builder()
        .label(subtitle)
        .css_classes(["caption", "dim-label"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    row.append(&title_label);
    row.append(&sub_label);

    // Stash item list in an Rc<RefCell> so the update fn can mutate
    // it across calls. The DropDown is built from a StringList,
    // which we replace wholesale when the list changes.
    let items: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(initial_items.clone()));
    let item_refs: Vec<&str> = initial_items.iter().map(|s| s.as_str()).collect();
    let string_list = gtk4::StringList::new(&item_refs);
    let dropdown = gtk4::DropDown::builder()
        .model(&string_list)
        .hexpand(true)
        .margin_top(3)
        .build();

    // Set initial selection if present.
    if let Some(idx) = initial_items.iter().position(|s| s == initial_selection) {
        dropdown.set_selected(idx as u32);
    }

    // Wire selection-changed to on_change.
    let on_change = Rc::new(on_change);
    let items_for_changed = items.clone();
    {
        let on_change = on_change.clone();
        dropdown.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            let it = items_for_changed.borrow();
            if let Some(name) = it.get(idx) {
                on_change(name.clone());
            }
        });
    }
    row.append(&dropdown);

    // Build the update_fn closure. Captures the dropdown + string
    // list + items vec; replacing the model is the canonical way
    // to refresh a DropDown's options.
    let dropdown_for_update = dropdown.clone();
    let items_for_update = items.clone();
    let on_change_for_update = on_change.clone();
    let update_fn: Rc<dyn Fn(Vec<String>, String)> = Rc::new(
        move |new_items: Vec<String>, desired_selection: String| {
            if new_items.is_empty() {
                return;
            }
            // Dedupe while preserving order.
            let mut seen = std::collections::HashSet::new();
            let mut merged: Vec<String> = Vec::with_capacity(new_items.len());
            // Always include the desired_selection first if it
            // isn't already in new_items, so the user's saved model
            // remains pickable even when not currently installed.
            if !desired_selection.is_empty()
                && !new_items.iter().any(|s| s == &desired_selection)
            {
                merged.push(desired_selection.clone());
                seen.insert(desired_selection.clone());
            }
            for item in new_items {
                if seen.insert(item.clone()) {
                    merged.push(item);
                }
            }
            // Rebuild the StringList. GTK4 doesn't have an "append"
            // for StringList that updates the DropDown live, but
            // setting a fresh StringList works.
            let refs: Vec<&str> = merged.iter().map(|s| s.as_str()).collect();
            let new_list = gtk4::StringList::new(&refs);
            dropdown_for_update.set_model(Some(&new_list));
            *items_for_update.borrow_mut() = merged.clone();
            if let Some(idx) = merged.iter().position(|s| s == &desired_selection) {
                dropdown_for_update.set_selected(idx as u32);
            }
            tracing::debug!(
                "settings: model dropdown refreshed with {} items",
                merged.len()
            );
            // Don't call on_change on refresh — the selection
            // didn't change as far as the user is concerned.
            let _ = &on_change_for_update;
        },
    );

    DropdownRow {
        widget: row,
        update_fn,
    }
}

/// Compose a starter list of plausible LLM model names. Used as the
/// initial dropdown contents before the async Ollama fetch returns,
/// so the user always sees something even on first launch.
fn build_initial_model_list(current_model: &str) -> Vec<String> {
    let mut items: Vec<String> = vec![
        // Sensible defaults that work with Ollama out-of-the-box.
        "llama3.2:1b".into(),
        "llama3.2:3b".into(),
        "llama3.1:8b".into(),
        "qwen2.5:0.5b".into(),
        "qwen2.5:1.5b".into(),
        "qwen2.5:3b".into(),
        "gemma2:2b".into(),
        "phi3:mini".into(),
    ];
    // Make sure the user's current selection is in the list.
    if !current_model.is_empty()
        && !items.iter().any(|s| s == current_model)
    {
        items.insert(0, current_model.to_string());
    }
    items
}

/// Kick off an async fetch against the Ollama `/api/tags` endpoint
/// to enumerate installed models. On success, calls `update_fn` on
/// the GTK main thread with the model name list. Failures are
/// logged but silent to the user — the dropdown stays seeded with
/// its initial defaults.
///
/// `endpoint` is the configured LLM endpoint URL (e.g.
/// "http://localhost:11434/v1/chat/completions"). We strip the
/// `/v1/...` suffix to derive the API base (`/api/tags`).
fn spawn_ollama_model_fetch(
    endpoint: String,
    desired_selection: String,
    update_fn: Rc<dyn Fn(Vec<String>, String)>,
) {
    // Derive the /api/tags URL from the configured endpoint.
    // Common Ollama endpoint shapes:
    //   http://localhost:11434/v1/chat/completions    (OpenAI compat)
    //   http://localhost:11434/v1                     (OpenAI base)
    //   http://localhost:11434/api/chat               (native)
    //   http://localhost:11434                        (base)
    // Strategy: take everything up to the host:port, then append
    // /api/tags. This is robust across the common variants.
    let tags_url = derive_ollama_tags_url(&endpoint);
    if tags_url.is_empty() {
        tracing::debug!(
            "ollama model fetch: couldn't derive tags URL from endpoint {:?}",
            endpoint
        );
        return;
    }

    // Cross-thread channel; the worker thread runs the HTTP fetch
    // and the GTK main loop polls for the result.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
    let tags_url_for_thread = tags_url.clone();
    std::thread::spawn(move || {
        match fetch_ollama_models(&tags_url_for_thread) {
            Ok(models) if !models.is_empty() => {
                tracing::info!(
                    "ollama model fetch: got {} models from {}",
                    models.len(),
                    tags_url_for_thread
                );
                let _ = tx.send(models);
            }
            Ok(_) => {
                tracing::debug!(
                    "ollama model fetch: endpoint returned empty model list"
                );
            }
            Err(e) => {
                tracing::debug!(
                    "ollama model fetch: failed for {}: {}",
                    tags_url_for_thread,
                    e
                );
            }
        }
    });

    // Poll the receiver on the GTK main loop. Stop polling once
    // we get a result or after ~5 seconds.
    let start = std::time::Instant::now();
    let timeout_dur = std::time::Duration::from_secs(5);
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        if let Ok(models) = rx.try_recv() {
            update_fn(models, desired_selection.clone());
            return glib::ControlFlow::Break;
        }
        if start.elapsed() > timeout_dur {
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

/// Strip a chat-completion-style endpoint URL down to the base and
/// append `/api/tags`. Returns empty string on parse failure.
fn derive_ollama_tags_url(endpoint: &str) -> String {
    if endpoint.is_empty() {
        return String::new();
    }
    // Find scheme://host[:port] prefix. Everything after that is
    // the path we'll discard.
    let after_scheme = match endpoint.find("://") {
        Some(idx) => &endpoint[idx + 3..],
        None => return String::new(),
    };
    let scheme = &endpoint[..endpoint.find("://").unwrap()];
    let host_part = match after_scheme.find('/') {
        Some(slash) => &after_scheme[..slash],
        None => after_scheme,
    };
    format!("{}://{}/api/tags", scheme, host_part)
}

/// Fetch the model list from an Ollama `/api/tags` URL. Returns
/// a vec of model names sorted alphabetically. Uses reqwest's
/// blocking client with a short timeout — we don't want settings-
/// page open to hang on an unresponsive endpoint.
#[cfg(feature = "llm")]
fn fetch_ollama_models(url: &str) -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| format!("client build: {}", e))?;
    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("request: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().map_err(|e| format!("parse: {}", e))?;
    // Response shape: { "models": [{ "name": "llama3.2:1b", ... }, ...] }
    let models_arr = body
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "no `models` array in response".to_string())?;
    let mut names: Vec<String> = models_arr
        .iter()
        .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(not(feature = "llm"))]
fn fetch_ollama_models(_url: &str) -> Result<Vec<String>, String> {
    Err("llm feature not enabled".into())
}

/// Build the color palette section. Each "slot" is a labeled row with a
/// swatch button that opens a color chooser on tap.
fn build_color_section(
    cfg_state: Rc<RefCell<Config>>,
    notify: Rc<dyn Fn()>,
) -> gtk4::Box {
    let container = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .build();

    // (label, get-current-hex, set-new-hex). Function pointers avoid
    // capturing closures and keep `slots` Send-friendly for future
    // refactors.
    type Slot = (
        &'static str,
        fn(&Config) -> String,
        fn(&mut Config, String),
    );
    let slots: Vec<Slot> = vec![
        (
            "Background",
            |c| c.appearance.colors.background.clone(),
            |c, v| c.appearance.colors.background = v,
        ),
        (
            "Key surface",
            |c| c.appearance.colors.key_fill.clone(),
            |c, v| c.appearance.colors.key_fill = v,
        ),
        (
            "Key pressed",
            |c| c.appearance.colors.key_fill_pressed.clone(),
            |c, v| c.appearance.colors.key_fill_pressed = v,
        ),
        (
            "Key text",
            |c| c.appearance.colors.key_text.clone(),
            |c, v| c.appearance.colors.key_text = v,
        ),
        (
            "Modifier keys",
            |c| c.appearance.colors.modifier_fill.clone(),
            |c, v| c.appearance.colors.modifier_fill = v,
        ),
        (
            "Special keys",
            |c| c.appearance.colors.special_fill.clone(),
            |c, v| c.appearance.colors.special_fill = v,
        ),
        (
            "Accent",
            |c| c.appearance.colors.accent.clone(),
            |c, v| c.appearance.colors.accent = v,
        ),
        (
            "Swipe trail",
            |c| c.appearance.colors.trail.clone(),
            |c, v| c.appearance.colors.trail = v,
        ),
    ];

    for (label, getter, setter) in slots {
        // Wrap each color row in an Expander so the (expensive)
        // swatch grid only builds when the user actually wants
        // to change that color. With 8 colors × ~30 swatches each,
        // eagerly building this section is ~240 widgets + signal
        // connections — the dominant cost of settings-page load.
        //
        // The expander label shows a small color preview (current
        // value) so the user can see what's set without opening.
        let cfg_state_for_row = cfg_state.clone();
        let notify_for_row = notify.clone();
        let expander = gtk4::Expander::builder()
            .label(label)
            .css_classes(["color-section-expander"])
            .build();

        // Lazy-build: the inner FlowBox is constructed on the
        // FIRST expand transition and cached. Subsequent expand/
        // collapse cycles reuse the same widget tree.
        let built: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let expander_clone = expander.clone();
        let built_clone = built.clone();
        expander.connect_expanded_notify(move |exp| {
            if exp.is_expanded() && !built_clone.get() {
                built_clone.set(true);
                let row = build_color_row(
                    label,
                    cfg_state_for_row.clone(),
                    notify_for_row.clone(),
                    getter,
                    setter,
                );
                expander_clone.set_child(Some(&row));
            }
        });
        container.append(&expander);
    }

    // "Reset to theme defaults" button — empties every hex string so
    // the theme falls back to its built-in defaults at render time.
    let reset_btn = gtk4::Button::builder()
        .label("Reset colors to theme defaults")
        .css_classes(["destructive-action", "pill"])
        .margin_top(6)
        .build();
    {
        let cfg_state = cfg_state.clone();
        let notify = notify.clone();
        reset_btn.connect_clicked(move |_| {
            cfg_state.borrow_mut().appearance.colors =
                crate::config::ColorPalette::default();
            info!("settings: colors reset to theme defaults");
            notify();
        });
    }
    container.append(&reset_btn);

    container
}

/// One row in the color section: label + a horizontal strip of preset
/// color swatches the user can tap inline. NO modal color-chooser
/// dialog — that would close the keyboard surface on phone (since the
/// dialog opens as its own toplevel window, taking focus). This way the
/// pick happens entirely inside the keyboard's layer surface.
///
/// We sacrifice continuous-hue picking for accessibility-on-phone:
/// 14 curated colors covering the common needs (neutrals + saturated
/// hues + accent picks) plus a "default" reset button. If the user
/// wants something off-palette they can edit `~/.config/rusty-board/
/// config.toml` directly.
/// Static palette shared by every color row. The first entry is the
/// "Default" pseudo-color (empty hex string = "fall back to theme");
/// the rest are curated presets ordered as neutrals → warm → cool.
/// Color palette: vibrant primary colors in the top row, lighter
/// muted alternatives in the bottom row. The same number of color
/// "slots" as before; the layout is two rows of 13 swatches each
/// (Default + Black + 11 hues × 2 rows = 24 muted + 1 default + 1
/// neutral row of grays).
///
/// Pairs are arranged so the vibrant version sits directly above
/// its muted version in the FlowBox grid (achieved by setting
/// min_children_per_line = number of colors and laying them out as
/// vibrant-row then muted-row, like a 2D table flattened to row-major).
const SWATCH_PALETTE: &[(&str, &str)] = &[
    // Row 1: defaults + vibrant
    ("Default", ""),
    ("Black", "1a1a1a"),
    ("Dark gray", "333333"),
    ("Med gray", "5a5a5a"),
    ("Light gray", "9a9a9a"),
    ("White", "fafafa"),
    ("Red", "e01b24"),
    ("Orange", "ff7800"),
    ("Yellow", "f5c211"),
    ("Green", "33d17a"),
    ("Teal", "1abc9c"),
    ("Blue", "3584e4"),
    ("Indigo", "5e54c9"),
    ("Purple", "9141ac"),
    ("Pink", "e668c9"),
    // Row 2: muted versions of the same hues (about 60% saturation,
    // 80% lightness). Gives users softer alternatives without
    // doubling the row count. These are picked to harmonize with
    // dark-mode UI chrome.
    ("Muted black", "2c2c2c"),
    ("Muted dark gray", "454545"),
    ("Muted med gray", "707070"),
    ("Muted light gray", "b3b3b3"),
    ("Muted off-white", "ebebeb"),
    ("Muted red", "d68080"),
    ("Muted orange", "e0a070"),
    ("Muted yellow", "ddc977"),
    ("Muted green", "8fcfa6"),
    ("Muted teal", "8fc9b8"),
    ("Muted blue", "8fb3d9"),
    ("Muted indigo", "9b96c9"),
    ("Muted purple", "b08fb8"),
    ("Muted pink", "d9b3cf"),
];

/// CSS for every swatch class. Installed once globally rather than 14
/// times per row to keep the panel snappy. Each color gets a class
/// `.color-swatch-<hex>`. The "Default" swatch has its own class with
/// a checkerboard look to communicate "no override".
///
/// Called once at panel construction. Safe to call multiple times —
/// `add_provider_for_display` deduplicates by provider identity, and
/// we use a static cell so the same provider gets registered once
/// across the process.
fn install_swatch_css_once() {
    use std::sync::OnceLock;
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let mut css = String::new();
        // Base swatch style — applies to all swatches regardless of color.
        css.push_str(
            ".color-swatch { \
                border: 1px solid alpha(@borders, 0.5); \
                border-radius: 4px; \
                min-width: 16px; \
                min-height: 16px; \
                padding: 0; \
            } \n\
            .color-swatch.selected { \
                border: 2px solid @accent_color; \
                min-width: 20px; \
                min-height: 20px; \
            } \n\
            .color-swatch-default { \
                background-image: linear-gradient( \
                    45deg, \
                    alpha(@window_fg_color, 0.15) 25%, \
                    transparent 25%, \
                    transparent 50%, \
                    alpha(@window_fg_color, 0.15) 50%, \
                    alpha(@window_fg_color, 0.15) 75%, \
                    transparent 75%); \
                background-size: 6px 6px; \
            }\n",
        );
        // Per-color background rules. Skip the "Default" pseudo-entry
        // since its style is via the checkerboard class above.
        for (_name, hex) in SWATCH_PALETTE.iter().filter(|(_, h)| !h.is_empty()) {
            css.push_str(&format!(
                ".color-swatch-{} {{ background-color: #{}; }}\n",
                hex, hex
            ));
        }
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(&css);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

fn build_color_row(
    label: &'static str,
    cfg_state: Rc<RefCell<Config>>,
    notify: Rc<dyn Fn()>,
    getter: fn(&Config) -> String,
    setter: fn(&mut Config, String),
) -> gtk4::Box {
    // The `label` parameter is kept for diagnostic logging (which
    // color was changed) but is no longer rendered — the Expander
    // wrapping this row shows it.
    install_swatch_css_once();

    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(4)
        .margin_top(4)
        .build();

    let swatch_grid = gtk4::FlowBox::builder()
        .min_children_per_line(15)
        .max_children_per_line(15)
        .row_spacing(3)
        .column_spacing(3)
        .selection_mode(gtk4::SelectionMode::None)
        .homogeneous(true)
        .build();

    // Track all swatch buttons so we can re-style which one is
    // selected when the user clicks a new one. Keyed by hex (empty
    // string for the "Default" swatch).
    let swatches: Rc<RefCell<Vec<(String, gtk4::Button)>>> =
        Rc::new(RefCell::new(Vec::with_capacity(SWATCH_PALETTE.len())));

    let current_hex = getter(&*cfg_state.borrow());

    for (name, hex) in SWATCH_PALETTE {
        let hex_owned = hex.to_string();
        let name_owned: String = (*name).to_string();
        let is_default = hex.is_empty();
        let is_selected = current_hex == *hex;

        // Build the per-swatch class list. All swatches get `.color-swatch`;
        // each non-default also gets its `.color-swatch-<hex>` class.
        // The "Default" swatch gets `.color-swatch-default` for its
        // checkerboard look. Selected swatches get `.selected`.
        let mut classes: Vec<String> = vec!["color-swatch".into()];
        if is_default {
            classes.push("color-swatch-default".into());
        } else {
            classes.push(format!("color-swatch-{}", hex));
        }
        if is_selected {
            classes.push("selected".into());
        }
        let class_refs: Vec<&str> = classes.iter().map(|s| s.as_str()).collect();

        let swatch = gtk4::Button::builder()
            .label("")
            .css_classes(class_refs.as_slice())
            .tooltip_text(if is_default {
                "Use theme default".to_string()
            } else {
                format!("{} (#{})", name, hex)
            })
            .build();

        swatches
            .borrow_mut()
            .push((hex_owned.clone(), swatch.clone()));

        let cfg_state_cb = cfg_state.clone();
        let notify_cb = notify.clone();
        let swatches_cb = swatches.clone();
        swatch.connect_clicked(move |_| {
            setter(&mut *cfg_state_cb.borrow_mut(), hex_owned.clone());
            info!(
                "settings: color {} -> {} (\"{}\")",
                label, name_owned, hex_owned
            );
            // Re-render selected state on all swatches in this row.
            let new_hex = &hex_owned;
            for (hex_other, btn_other) in swatches_cb.borrow().iter() {
                let mut cls: Vec<String> = btn_other
                    .css_classes()
                    .iter()
                    .map(|s| s.to_string())
                    .filter(|c| c != "selected")
                    .collect();
                if hex_other == new_hex {
                    cls.push("selected".into());
                }
                let refs: Vec<&str> = cls.iter().map(|s| s.as_str()).collect();
                btn_other.set_css_classes(&refs);
            }
            notify_cb();
        });
        swatch_grid.append(&swatch);
    }

    row.append(&swatch_grid);
    row
}

/// Convert a hex string (no `#`) to a gdk::RGBA. Returns None for empty
/// or malformed input. Kept for future use; the inline-swatch picker
/// doesn't need it but the helper is small and may be useful again
/// (e.g. when validating user-edited config.toml hex values).
#[allow(dead_code)]
fn parse_hex_to_rgba(hex: &str) -> Option<gtk4::gdk::RGBA> {
    if hex.is_empty() {
        return None;
    }
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(gtk4::gdk::RGBA::new(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        1.0,
    ))
}

/// Convert a gdk::RGBA to a 6-char hex (no leading `#`). See
/// `parse_hex_to_rgba` re: why we keep this around.
#[allow(dead_code)]
fn rgba_to_hex(rgba: &gtk4::gdk::RGBA) -> String {
    format!(
        "{:02x}{:02x}{:02x}",
        (rgba.red() * 255.0).round() as u8,
        (rgba.green() * 255.0).round() as u8,
        (rgba.blue() * 255.0).round() as u8,
    )
}

fn row_style_index(cfg: &Config) -> usize {
    if cfg.layout.row_style == "ortholinear" {
        1
    } else {
        0
    }
}

fn letters_index(cfg: &Config) -> usize {
    if cfg.layout.letters == "dvorak" {
        1
    } else {
        0
    }
}

fn experimental_mode_index(cfg: &Config) -> usize {
    match cfg.layout.experimental_mode.as_str() {
        "cardinal" => 1,
        "stenotype" => 2,
        "cardinal_words" => 3,
        _ => 0,
    }
}

/// Build a default thumb-arc polygon for a freshly-enabled
/// single-hand-mode session. Uses the primary monitor's logical width
/// to scale the compaction factor (bigger screens compact more
/// aggressively since their far corners are harder to reach).
///
/// If we can't query the display (e.g. headless test), default to
/// 360px which is a typical phone width and gives mild compaction.
fn default_arc_for_screen(
    anchor: crate::config::ThumbAnchor,
) -> crate::config::ThumbArc {
    use gtk4::prelude::*;
    let surface_w = gtk4::gdk::Display::default()
        .and_then(|display| display.monitors().item(0))
        .and_then(|monitor| monitor.downcast::<gtk4::gdk::Monitor>().ok())
        .map(|m| m.geometry().width() as f64)
        .unwrap_or(360.0);
    crate::config::ThumbArc::default_for_width(anchor, surface_w)
}
