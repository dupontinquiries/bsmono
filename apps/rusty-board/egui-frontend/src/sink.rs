//! sink — where a keypress actually goes.
//!
//! ## The demo/keyboard gap
//!
//! `KeyboardService::commit_text` pushes onto a `pending` queue and
//! `take_pending` drains it. Nothing in the front-end called
//! `take_pending` — only its own tests did. So every tap, swipe word and
//! dictation result was routed correctly into a `Vec<String>` that was
//! never read, and the only visible effect was `self.typed`, a preview
//! string drawn inside the app's own window.
//!
//! That is what made this "a demo rather than a keyboard": it is a
//! faithful simulation of typing, into itself. On a desktop that is
//! testable and useful; on a Fairphone under Plasma Mobile it is a
//! keyboard that types nowhere.
//!
//! ## What was already there
//!
//! Nearly everything. `board_compositor::virtual_keyboard` has a
//! `VirtualKeyboard` trait with `commit_text` / `press_keysym`, backend
//! DETECTION (`detect()`), and a Wayland implementation speaking
//! `zwp_input_method_v2` / `zwp_virtual_keyboard_v1`. The egui front-end
//! simply never reached for it — it is a separate crate that this one
//! does not depend on.
//!
//! ## Why a trait here rather than depending on that crate
//!
//! Because the front-end's own description promises it stays "buildable
//! on its own", and a hard dependency on the Wayland stack would break
//! that — including on this developer machine, where there is no
//! compositor to talk to.
//!
//! So: a two-line trait the front-end owns, a [`Preview`] implementation
//! for desktop and tests, and a real one constructed on device. The
//! keyboard drives a `dyn Sink` and does not know which it has, which is
//! also what makes the typing path testable without a compositor.

/// Where committed text and synthetic keys go.
pub trait Sink {
    /// Literal text: a tap on `a`, a swiped word, a dictation result.
    fn commit_text(&mut self, text: &str) -> Result<(), String>;

    /// A non-character key by X keysym name: `BackSpace`, `Return`,
    /// `Tab`, `Left`. Named rather than numbered because that is what
    /// the Wayland backend already takes, and inventing a second
    /// spelling would mean a translation table to keep in step.
    fn press_keysym(&mut self, name: &str) -> Result<(), String>;

    /// For the status line, so a person can tell a keyboard that is
    /// typing into the world from one typing into itself. That
    /// distinction was invisible before, which is most of why this went
    /// unnoticed.
    fn describe(&self) -> String;

    /// Drive whatever background event processing this sink needs
    /// (e.g. dispatching a Wayland event queue). Called once per frame
    /// from `demo_app`'s `update()`, regardless of which sink is active
    /// — a no-op default means most sinks don't need to know this
    /// exists.
    fn pump(&mut self) {}

    /// A descriptor that is readable when `pump` has work. The layer shell
    /// polls it next to its own socket so an idle keyboard sleeps instead of
    /// waking on a timer to ask. `None`: nothing to wait on.
    fn wake_fd(&self) -> Option<std::os::fd::RawFd> {
        None
    }

    /// What this sink knows about focus. The DECISION about visibility
    /// is not made here — see `focus_policy`.
    ///
    /// This used to be `fn is_visible(&self) -> bool`, defaulting to
    /// `true`. The bool was the bug: it gave one value to two states that
    /// need different handling — "there is no focus signal, so never
    /// hide" (the `Preview` default) and "there is a signal and it says
    /// no field is focused". The Wayland sink's signal starts at
    /// not-focused and stays there forever if the compositor never
    /// activates it, so a keyboard that hid on `false` hid permanently
    /// with nothing in the log to say why.
    fn focus_source(&self) -> crate::focus_policy::FocusSource {
        crate::focus_policy::FocusSource::None
    }
}

/// The desktop/test sink: remembers everything, sends nothing.
///
/// This is what the demo was doing implicitly. Made EXPLICIT so it has a
/// name, shows up in the status line, and cannot be mistaken for the
/// real thing.
#[derive(Default, Debug)]
pub struct Preview {
    pub text: String,
    pub keys: Vec<String>,
}

impl Sink for Preview {
    fn commit_text(&mut self, text: &str) -> Result<(), String> {
        self.text.push_str(text);
        Ok(())
    }

    fn press_keysym(&mut self, name: &str) -> Result<(), String> {
        // Applied to the preview too, so what the box shows matches what
        // a real sink would have produced. A backspace that visibly did
        // nothing in the demo would have hidden a real bug in the key
        // routing.
        match name {
            "BackSpace" => {
                self.text.pop();
            }
            "Return" => self.text.push('\n'),
            "Tab" => self.text.push('\t'),
            "space" => self.text.push(' '),
            _ => {}
        }
        self.keys.push(name.to_string());
        Ok(())
    }

    fn describe(&self) -> String {
        "preview (typing into this window, not the system)".into()
    }
}

/// Choose a sink.
///
/// Returns [`Preview`] when there is no compositor to talk to, which is
/// the desktop-development case and must stay working — the front-end's
/// whole reason for existing separately is that it builds and runs
/// without the Wayland stack.
///
/// The real path is behind the `wayland` feature and constructed by
/// [`crate::sink::system`], so a build without it cannot silently
/// produce a keyboard that appears to type and does not.
pub fn detect() -> Box<dyn Sink> {
    #[cfg(feature = "wayland")]
    {
        match system() {
            Ok(s) => return s,
            Err(e) => {
                // Loud, once, on stderr: a keyboard that quietly falls
                // back to typing into itself is exactly the failure this
                // module exists to end.
                eprintln!("[rusty-board] no input-method backend ({e}) — preview only");
            }
        }
    }
    Box::new(Preview::default())
}

/// The real sink, on a Wayland session with an input-method protocol.
///
/// Delegates entirely to [`crate::wayland::WaylandIntegration`], which
/// also owns show/hide (see that module) — a bare
/// `VirtualKeyboardBackend::detect().build()` deliberately returns an
/// EMPTY Wayland backend with no input-method callback wired (see that
/// function's own doc comment in `virtual_keyboard.rs`); getting the
/// real one requires going through `wayland_backend::try_new_standalone`
/// directly, which is what `WaylandIntegration::try_new` does.
#[cfg(feature = "wayland")]
pub fn system() -> Result<Box<dyn Sink>, String> {
    let integration = crate::wayland::WaylandIntegration::try_new()?;
    Ok(Box::new(integration))
}

#[cfg(feature = "wayland")]
impl Sink for crate::wayland::WaylandIntegration {
    fn commit_text(&mut self, text: &str) -> Result<(), String> {
        self.vk.commit_text(text).map_err(|e| e.to_string())
    }
    fn press_keysym(&mut self, name: &str) -> Result<(), String> {
        self.vk.press_keysym(name).map_err(|e| e.to_string())
    }
    fn describe(&self) -> String {
        format!("{} (typing into the focused window)", self.vk.backend_name())
    }
    fn pump(&mut self) {
        crate::wayland::WaylandIntegration::pump(self);
    }
    fn wake_fd(&self) -> Option<std::os::fd::RawFd> {
        self.vk.poll_fd()
    }
    fn focus_source(&self) -> crate::focus_policy::FocusSource {
        crate::focus_policy::FocusSource::InputMethod {
            active: crate::wayland::WaylandIntegration::is_active(self),
            unavailable: board_compositor::im_health::input_method_unavailable(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_applies_editing_keys_not_just_text() {
        // A backspace that visibly does nothing in the demo would hide a
        // real bug in the key routing — the preview has to behave like
        // the thing it stands in for.
        let mut p = Preview::default();
        p.commit_text("hel").unwrap();
        p.commit_text("lop").unwrap();
        p.press_keysym("BackSpace").unwrap();
        assert_eq!(p.text, "hello");
        p.press_keysym("space").unwrap();
        p.commit_text("world").unwrap();
        assert_eq!(p.text, "hello world");
    }

    #[test]
    fn the_preview_says_it_is_a_preview() {
        // The distinction between typing into the world and typing into
        // this window was invisible, which is most of why the gap went
        // unnoticed for so long.
        let p = Preview::default();
        assert!(p.describe().contains("not the system"), "{}", p.describe());
    }

    #[test]
    fn keysyms_are_recorded_even_when_the_preview_ignores_them() {
        // An unhandled keysym must still be observable, or a test cannot
        // tell "routed and ignored" from "never routed" — which is the
        // exact confusion this whole module is about.
        let mut p = Preview::default();
        p.press_keysym("Left").unwrap();
        assert_eq!(p.keys, vec!["Left".to_string()]);
        assert!(p.text.is_empty());
    }
}
