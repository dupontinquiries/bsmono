//! Individual key widget — a styled `Button` with built-in animation.
//!
//! We don't subclass `gtk4::Button` (which would require unsafe glib subclass
//! plumbing); we just attach state and gestures to a regular Button. The
//! parent `KeyboardView` owns the engine and will route presses through.

use crate::keyboard::{Key, KeyAction, KeyType, KeypadState};
use crate::ui::animations::Animator;
use gtk4::prelude::*;

#[derive(Clone)]
pub struct KeyWidget {
    button: gtk4::Button,
    label: gtk4::Label,
    key: Key,
}

impl KeyWidget {
    pub fn new(key: Key) -> Self {
        // Tack on steno-specific classes when this is a steno key.
        // The visible parallelogram-ish shape comes from the
        // .key-steno-L / .key-steno-R CSS rules in keyboard.css.
        let mut classes: Vec<&'static str> =
            vec!["key-button", css_class_for_type(&key.key_type)];
        if let KeyAction::Steno(id) = &key.action {
            classes.push("key-steno");
            classes.push(if id.starts_with('L') {
                "key-steno-L"
            } else {
                "key-steno-R"
            });
        }
        let button = gtk4::Button::builder()
            .css_classes(classes)
            .hexpand(true)
            .vexpand(true)
            .build();
        // Layout-author-supplied extra class (e.g. "arcade-buffer"
        // on the thin Dvorak spacebar flanks). Applied after build
        // to keep the build-time classes vec a Vec<&'static str> —
        // most keys have no extra class and don't allocate.
        if let Some(extra) = &key.extra_css_class {
            button.add_css_class(extra);
        }

        // For Voice and LLM keys, render a clean grayscale SVG
        // icon instead of an emoji glyph. Emoji rendering varies
        // wildly across systems (color emoji fonts, monochrome
        // fallback, missing glyph boxes); a bundled SVG gives a
        // consistent look. The icon uses `currentColor` so CSS
        // controls the actual color — by default symbolic gray,
        // tinted to accent when the user starts an action.
        //
        // We still keep a `label` member set to the original
        // emoji text. State-tracking code (update_for_state) and
        // any consumer that compares key.label by string keeps
        // working unchanged.
        let label = gtk4::Label::builder()
            .label(&key.label)
            .css_classes(["key-label"])
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .build();

        // Decide what to use as the button's child.
        let icon_widget: Option<gtk4::Widget> = match key.action {
            KeyAction::Voice => Some(build_voice_icon().upcast()),
            KeyAction::Llm => Some(build_magic_icon().upcast()),
            _ => None,
        };

        if let Some(icon) = icon_widget {
            // Wrap the icon in a Box so we can vertical-center it
            // the same way the label is centered. The button's
            // native child sizing isn't always Centered for raw
            // images.
            let centered = gtk4::Box::builder()
                .orientation(gtk4::Orientation::Vertical)
                .halign(gtk4::Align::Center)
                .valign(gtk4::Align::Center)
                .build();
            centered.append(&icon);
            button.set_child(Some(&centered));
        } else if let Some(gestures) = key.gestures.as_ref().filter(|g| {
            g.n.is_some() || g.e.is_some() || g.s.is_some() || g.w.is_some()
        }) {
            let grid = gtk4::Grid::builder()
                .row_homogeneous(true)
                .column_homogeneous(true)
                .hexpand(true)
                .vexpand(true)
                .build();
            grid.attach(&label, 1, 1, 1, 1);
            // Attach hints OR invisible spacers to every cardinal
            // slot. Without spacers in empty slots, homogeneous
            // sizing only operates over the rows/columns that
            // actually have content — so a cell with only an `n`
            // hint becomes a 2-row grid with the center label
            // visually at the bottom row. Spacers force the grid
            // to lay out as a full 3×3, with the center label
            // truly centered.
            //
            // The spacer is a Label with empty text and the
            // appropriate hint class so it picks up the same
            // sizing CSS as a real hint would.
            for (dir, content, col, row) in [
                ("hint-n", gestures.n.as_deref(), 1, 0),
                ("hint-e", gestures.e.as_deref(), 2, 1),
                ("hint-s", gestures.s.as_deref(), 1, 2),
                ("hint-w", gestures.w.as_deref(), 0, 1),
            ] {
                match content {
                    Some(c) => grid.attach(&hint_label(c, dir), col, row, 1, 1),
                    None => grid.attach(&hint_spacer(dir), col, row, 1, 1),
                }
            }
            button.set_child(Some(&grid));
        } else {
            button.set_child(Some(&label));
        }

        Self {
            button,
            label,
            key,
        }
    }

    pub fn widget(&self) -> &gtk4::Button {
        &self.button
    }

    pub fn key(&self) -> &Key {
        &self.key
    }

    /// Update the displayed label according to the current keyboard state.
    pub fn update_for_state(&self, state: KeypadState) {
        let shifted = state.is_shifted();
        let label = self.key.display_label(shifted);
        self.label.set_label(label);

        // Visual indicator for caps-lock on shift modifier keys.
        // We use add_css_class / remove_css_class directly rather than
        // round-tripping through css_classes() → Vec<String> → filter →
        // Vec<&str> → set_css_classes(). The old path allocated several
        // vecs and cloned every existing class to a String just to
        // toggle one or two of them; the direct calls are
        // allocation-free single GLib operations.
        //
        // Auto-cap fires this on every cursor-position change on
        // edit-focused fields, so this is a steady-state cost during
        // typing — not just on explicit shift taps. Multiply the
        // savings by the number of shift keys on the layout.
        if matches!(self.key.action, KeyAction::Shift) {
            // Always remove BOTH (cheap if absent — GTK no-ops the
            // remove if the class isn't on the widget) so we land in
            // a known state before re-adding for the current variant.
            self.button.remove_css_class("shift-active");
            self.button.remove_css_class("shift-locked");
            match state {
                KeypadState::Shifted => self.button.add_css_class("shift-active"),
                KeypadState::CapsLock => self.button.add_css_class("shift-locked"),
                _ => {}
            }
        }
    }

    /// Animate a press feedback flash.
    pub fn animate_press(&self) {
        Animator::key_press_pulse(&self.button);
    }

    /// Register a callback fired on click (release-inside-bounds).
    /// Used by the main keyboard tap pipeline where the click event
    /// arrives only after the user lifts their finger inside the key
    /// — by which time the swipe handler has already decided whether
    /// the gesture was a tap or a real swipe.
    ///
    /// For keys that should repeat while held (Backspace, arrow keys),
    /// use `add_press_release_callbacks` instead — that gives you
    /// press AND release signals so you can start/stop a repeat timer.
    pub fn add_pressed_callback<F>(&self, cb: F)
    where
        F: Fn(&Key) + 'static,
    {
        let key = self.key.clone();
        self.button.connect_clicked(move |_| {
            cb(&key);
        });
    }

    /// Register press AND release callbacks. The press fires the
    /// instant the finger touches the key (or mouse button presses);
    /// the release fires when the finger lifts. Used by the key-
    /// repeat logic for Backspace and arrow keys so a held key
    /// generates a stream of repeated commits, matching the behavior
    /// of a hardware keyboard.
    ///
    /// NOTE: pressing a key here also fires the regular click handler
    /// on release. Callers using both `add_pressed_callback` and
    /// `add_press_release_callbacks` will see the press callback,
    /// then the release callback, then the click callback in that
    /// order. Plan ordering accordingly.
    pub fn add_press_release_callbacks<P, R>(&self, on_press: P, on_release: R)
    where
        P: Fn(&Key) + 'static,
        R: Fn(&Key) + 'static,
    {
        // Use the button's own state-flags-changed signal as the
        // press/release source. `StateFlags::ACTIVE` is GTK's
        // representation of "this button is currently being
        // pressed" — set when the user touches the button, cleared
        // when they release OR when the gesture is cancelled
        // (parent grabs the touch, finger drifts out, surface
        // hidden). That's exactly what we want for repeat:
        //
        //   - Fires on EVERY entry to/exit from the pressed state
        //   - Works with touch and mouse identically
        //   - Doesn't fight the existing GestureClick logic the
        //     button uses for its own connect_clicked signal —
        //     this is the same underlying state machine
        //
        // Previously I tried adding a separate GestureClick
        // controller, which had subtle issues with GTK4's gesture-
        // claim semantics (cancel didn't always fire, and parent
        // swipe handlers would intercept release events). The
        // state-flags approach side-steps all of that.
        let key = self.key.clone();
        let on_press = std::rc::Rc::new(on_press);
        let on_release = std::rc::Rc::new(on_release);
        let was_active = std::rc::Rc::new(std::cell::Cell::new(false));
        let was_active_for_signal = was_active.clone();
        self.button.connect_state_flags_changed(move |btn, _prev_flags| {
            use gtk4::prelude::WidgetExt;
            let active_now = btn.state_flags().contains(gtk4::StateFlags::ACTIVE);
            let was = was_active_for_signal.get();
            if active_now == was {
                // Some other flag changed (hover, focus, etc).
                // Ignore — we only care about pressed/released.
                return;
            }
            was_active_for_signal.set(active_now);
            if active_now {
                on_press(&key);
            } else {
                on_release(&key);
            }
        });
    }
}

fn css_class_for_type(t: &KeyType) -> &'static str {
    match t {
        KeyType::Normal => "key-normal",
        KeyType::Modifier => "key-modifier",
        KeyType::Action => "key-action",
        KeyType::Special => "key-special",
    }
}

/// Build a small, greyed-out label for a cardinal-direction hint. The
/// extra CSS class (`hint-n`, `hint-e`, etc.) lets the stylesheet
/// fine-tune positioning (e.g. tiny margins) per direction if needed.
fn hint_label(text: &str, dir_class: &'static str) -> gtk4::Label {
    let halign = match dir_class {
        "hint-w" => gtk4::Align::Start,
        "hint-e" => gtk4::Align::End,
        _ => gtk4::Align::Center,
    };
    let valign = match dir_class {
        "hint-n" => gtk4::Align::Start,
        "hint-s" => gtk4::Align::End,
        _ => gtk4::Align::Center,
    };
    gtk4::Label::builder()
        .label(text)
        .css_classes(["key-hint", dir_class])
        .halign(halign)
        .valign(valign)
        .build()
}

/// Invisible placeholder used in empty cardinal-cell slots. Has the
/// same CSS class as a real hint label so it picks up identical
/// sizing rules — the grid's homogeneous layout then operates over
/// the full 3×3 cell space and centers the main label correctly.
///
/// The text is a single non-breaking space (so the label has a
/// non-zero natural size) and the widget is opacity:0 (so it's
/// invisible). Using opacity rather than `set_visible(false)`
/// because hidden widgets don't participate in grid sizing.
fn hint_spacer(dir_class: &'static str) -> gtk4::Label {
    let lbl = hint_label("\u{00a0}", dir_class);
    lbl.set_opacity(0.0);
    lbl
}

/// SVG bytes for the voice / microphone icon. Embedded at compile
/// time so the keyboard doesn't depend on filesystem state at
/// startup (some sandboxed launch paths run before $HOME or even
/// /usr/share is fully visible).
const VOICE_SVG: &[u8] = include_bytes!("../../resources/icons/voice.svg");

/// SVG bytes for the LLM / magic / sparkle icon.
const MAGIC_SVG: &[u8] = include_bytes!("../../resources/icons/magic.svg");

/// Build a gtk4::Image from inline SVG bytes via gdk::Texture
/// loading. We use Texture::from_bytes which understands SVG via
/// gdk-pixbuf (which links the librsvg loader at runtime).
///
/// The returned Image is sized to the standard key-icon dimensions
/// (20px logical, scales to ~60px actual on 3x HiDPI phone).
///
/// Textures are cached thread-locally — decoding the SVG runs
/// librsvg's parser and rasterizes the pixels, which is the
/// expensive part. With caching, only the first KeyWidget that
/// uses a given icon pays that cost; subsequent rebuilds reuse the
/// already-decoded GPU texture. gtk4::gdk::Texture is not Send,
/// but all KeyWidget creation happens on the main thread, so
/// thread_local fits cleanly.
fn build_icon_image(svg_bytes: &'static [u8], css_class: &str) -> gtk4::Image {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static TEXTURE_CACHE: RefCell<HashMap<usize, Option<gtk4::gdk::Texture>>> =
            RefCell::new(HashMap::new());
    }

    // Cache key: the &'static [u8] pointer is stable across calls
    // because the bytes come from include_bytes!. Using the data
    // pointer (.as_ptr() as usize) gives a unique key per icon.
    let cache_key = svg_bytes.as_ptr() as usize;

    let texture: Option<gtk4::gdk::Texture> = TEXTURE_CACHE.with(|cache| {
        let mut map = cache.borrow_mut();
        if let Some(entry) = map.get(&cache_key) {
            return entry.clone();
        }
        let bytes = glib::Bytes::from(svg_bytes);
        let decoded = match gtk4::gdk::Texture::from_bytes(&bytes) {
            Ok(texture) => Some(texture),
            Err(e) => {
                tracing::warn!("Failed to load icon SVG: {}", e);
                None
            }
        };
        map.insert(cache_key, decoded.clone());
        decoded
    });

    // Texture::from_bytes can fail if no SVG loader is available.
    // In that case we fall back to a blank Image — the button is
    // still tappable, just visually empty. This is better than
    // panicking during startup.
    let image = match texture {
        Some(t) => gtk4::Image::from_paintable(Some(&t)),
        None => gtk4::Image::new(),
    };
    image.set_pixel_size(20);
    image.add_css_class("key-icon");
    image.add_css_class(css_class);
    image
}

fn build_voice_icon() -> gtk4::Image {
    build_icon_image(VOICE_SVG, "key-icon-voice")
}

fn build_magic_icon() -> gtk4::Image {
    build_icon_image(MAGIC_SVG, "key-icon-magic")
}
