//! Backend-neutral input codes (T07-R27, glfw→winit migration, Approach B).
//!
//! `input.rs` and `keybindings.rs` historically depended on `glfw::Key`,
//! `glfw::MouseButton`, and `glfw::Action` directly. That hard-wires the whole
//! input + keybind system to glfw, which is exactly what blocks dropping the
//! `glfw-sys`/cmake build (see `docs/GLFW_DECOUPLING.md`).
//!
//! This module introduces a neutral keycode/button/action vocabulary that BOTH
//! the legacy glfw path and the new winit path translate INTO, so the rest of
//! the engine never names a backend key type again. It is intentionally pure
//! (no `ash`/`glfw`/`winit` in the core types) so it compiles and unit-tests
//! headlessly; the per-backend `From` conversions are feature-gated.
//!
//! ## Serialization stability
//! `keybindings.rs` persists binds as the key's `Debug` name (e.g. "Space").
//! `Keycode`'s variant names match the `glfw::Key` names one-for-one, and
//! [`Keycode::from_name`]/[`Keycode::name`] round-trip those same strings, so
//! existing `keybindings.json` files keep loading unchanged across the
//! migration. The stable `i32` repr ([`Keycode::to_i32`]/[`Keycode::from_i32`])
//! is provided for any callers that prefer a numeric scancode-style id.

/// A physical key, named after the US-QWERTY legend (matching the prior
/// `glfw::Key` naming so persisted binds stay compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
         serde::Serialize, serde::Deserialize)]
pub enum Keycode {
    // Letters
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    // Number row
    Num0, Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9,
    // Function keys
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    // Whitespace / editing
    Space, Tab, Enter, Backspace, Escape,
    // Punctuation
    Comma, Period, Slash, Semicolon, Apostrophe, Minus, Equal,
    LeftBracket, RightBracket, Backslash, GraveAccent,
    // Modifiers
    LeftShift, RightShift, LeftControl, RightControl,
    LeftAlt, RightAlt, LeftSuper, RightSuper,
    CapsLock, NumLock, ScrollLock,
    // Arrows + navigation
    Up, Down, Left, Right,
    Insert, Delete, Home, End, PageUp, PageDown,
}

impl Keycode {
    /// Every variant, in declaration order. Single source of truth for the
    /// name/i32 round-trips and the completeness test.
    pub const ALL: &'static [Keycode] = &[
        Keycode::A, Keycode::B, Keycode::C, Keycode::D, Keycode::E, Keycode::F,
        Keycode::G, Keycode::H, Keycode::I, Keycode::J, Keycode::K, Keycode::L,
        Keycode::M, Keycode::N, Keycode::O, Keycode::P, Keycode::Q, Keycode::R,
        Keycode::S, Keycode::T, Keycode::U, Keycode::V, Keycode::W, Keycode::X,
        Keycode::Y, Keycode::Z,
        Keycode::Num0, Keycode::Num1, Keycode::Num2, Keycode::Num3, Keycode::Num4,
        Keycode::Num5, Keycode::Num6, Keycode::Num7, Keycode::Num8, Keycode::Num9,
        Keycode::F1, Keycode::F2, Keycode::F3, Keycode::F4, Keycode::F5, Keycode::F6,
        Keycode::F7, Keycode::F8, Keycode::F9, Keycode::F10, Keycode::F11, Keycode::F12,
        Keycode::Space, Keycode::Tab, Keycode::Enter, Keycode::Backspace, Keycode::Escape,
        Keycode::Comma, Keycode::Period, Keycode::Slash, Keycode::Semicolon,
        Keycode::Apostrophe, Keycode::Minus, Keycode::Equal,
        Keycode::LeftBracket, Keycode::RightBracket, Keycode::Backslash, Keycode::GraveAccent,
        Keycode::LeftShift, Keycode::RightShift, Keycode::LeftControl, Keycode::RightControl,
        Keycode::LeftAlt, Keycode::RightAlt, Keycode::LeftSuper, Keycode::RightSuper,
        Keycode::CapsLock, Keycode::NumLock, Keycode::ScrollLock,
        Keycode::Up, Keycode::Down, Keycode::Left, Keycode::Right,
        Keycode::Insert, Keycode::Delete, Keycode::Home, Keycode::End,
        Keycode::PageUp, Keycode::PageDown,
    ];

    /// Stable, glfw-compatible string name (matches the old `Debug` of
    /// `glfw::Key`, which is what `keybindings.json` stores).
    pub fn name(self) -> &'static str {
        match self {
            Keycode::A => "A", Keycode::B => "B", Keycode::C => "C", Keycode::D => "D",
            Keycode::E => "E", Keycode::F => "F", Keycode::G => "G", Keycode::H => "H",
            Keycode::I => "I", Keycode::J => "J", Keycode::K => "K", Keycode::L => "L",
            Keycode::M => "M", Keycode::N => "N", Keycode::O => "O", Keycode::P => "P",
            Keycode::Q => "Q", Keycode::R => "R", Keycode::S => "S", Keycode::T => "T",
            Keycode::U => "U", Keycode::V => "V", Keycode::W => "W", Keycode::X => "X",
            Keycode::Y => "Y", Keycode::Z => "Z",
            Keycode::Num0 => "Num0", Keycode::Num1 => "Num1", Keycode::Num2 => "Num2",
            Keycode::Num3 => "Num3", Keycode::Num4 => "Num4", Keycode::Num5 => "Num5",
            Keycode::Num6 => "Num6", Keycode::Num7 => "Num7", Keycode::Num8 => "Num8",
            Keycode::Num9 => "Num9",
            Keycode::F1 => "F1", Keycode::F2 => "F2", Keycode::F3 => "F3", Keycode::F4 => "F4",
            Keycode::F5 => "F5", Keycode::F6 => "F6", Keycode::F7 => "F7", Keycode::F8 => "F8",
            Keycode::F9 => "F9", Keycode::F10 => "F10", Keycode::F11 => "F11", Keycode::F12 => "F12",
            Keycode::Space => "Space", Keycode::Tab => "Tab", Keycode::Enter => "Enter",
            Keycode::Backspace => "Backspace", Keycode::Escape => "Escape",
            Keycode::Comma => "Comma", Keycode::Period => "Period", Keycode::Slash => "Slash",
            Keycode::Semicolon => "Semicolon", Keycode::Apostrophe => "Apostrophe",
            Keycode::Minus => "Minus", Keycode::Equal => "Equal",
            Keycode::LeftBracket => "LeftBracket", Keycode::RightBracket => "RightBracket",
            Keycode::Backslash => "Backslash", Keycode::GraveAccent => "GraveAccent",
            Keycode::LeftShift => "LeftShift", Keycode::RightShift => "RightShift",
            Keycode::LeftControl => "LeftControl", Keycode::RightControl => "RightControl",
            Keycode::LeftAlt => "LeftAlt", Keycode::RightAlt => "RightAlt",
            Keycode::LeftSuper => "LeftSuper", Keycode::RightSuper => "RightSuper",
            Keycode::CapsLock => "CapsLock", Keycode::NumLock => "NumLock",
            Keycode::ScrollLock => "ScrollLock",
            Keycode::Up => "Up", Keycode::Down => "Down", Keycode::Left => "Left",
            Keycode::Right => "Right",
            Keycode::Insert => "Insert", Keycode::Delete => "Delete", Keycode::Home => "Home",
            Keycode::End => "End", Keycode::PageUp => "PageUp", Keycode::PageDown => "PageDown",
        }
    }

    /// Parse a stored name back into a `Keycode`. Unknown names return `None`
    /// (the caller drops the bind with a warning, as before).
    pub fn from_name(name: &str) -> Option<Keycode> {
        Keycode::ALL.iter().copied().find(|k| k.name() == name)
    }

    /// Stable numeric id (index into [`Keycode::ALL`]). Useful for any caller
    /// that wants a compact scancode-style integer instead of the string name.
    pub fn to_i32(self) -> i32 {
        Keycode::ALL.iter().position(|&k| k == self).unwrap() as i32
    }

    /// Inverse of [`Keycode::to_i32`].
    pub fn from_i32(v: i32) -> Option<Keycode> {
        usize::try_from(v).ok().and_then(|i| Keycode::ALL.get(i).copied())
    }
}

/// A mouse button. `Other` carries the platform button index for extra
/// buttons (side buttons etc.) so nothing is silently dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MouseBtn {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Key/button transition state. `Repeat` is OS auto-repeat of a held key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    Press,
    Release,
    Repeat,
}

/// Backend-neutral input event — the shape `input.rs` consumes. Both the glfw
/// flush loop and the winit event loop translate their native events into this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    Key { code: Keycode, action: KeyAction },
    /// A key the active layout produced that isn't in our `Keycode` table.
    /// Carried so the loop can count it without panicking; consumers ignore it.
    UnknownKey { action: KeyAction },
    MouseButton { button: MouseBtn, action: KeyAction },
    CursorPos { x: f64, y: f64 },
    Scroll { x: f64, y: f64 },
}

// =============================================================================
// glfw -> neutral (legacy backend). Gated so a glfw-free build never pulls it.
// =============================================================================
#[cfg(feature = "glfw")]
mod glfw_conv {
    use super::*;

    impl From<glfw::Action> for KeyAction {
        fn from(a: glfw::Action) -> Self {
            match a {
                glfw::Action::Press => KeyAction::Press,
                glfw::Action::Release => KeyAction::Release,
                glfw::Action::Repeat => KeyAction::Repeat,
            }
        }
    }

    impl From<glfw::MouseButton> for MouseBtn {
        fn from(b: glfw::MouseButton) -> Self {
            match b {
                glfw::MouseButton::Button1 => MouseBtn::Left,
                glfw::MouseButton::Button2 => MouseBtn::Right,
                glfw::MouseButton::Button3 => MouseBtn::Middle,
                glfw::MouseButton::Button4 => MouseBtn::Back,
                glfw::MouseButton::Button5 => MouseBtn::Forward,
                other => MouseBtn::Other(other as u16),
            }
        }
    }

    /// Map a `glfw::Key` to our neutral keycode. Returns `None` for keys we
    /// don't model (consumer treats them as `InputEvent::UnknownKey`).
    pub fn keycode_from_glfw(k: glfw::Key) -> Option<Keycode> {
        use glfw::Key as G;
        Some(match k {
            G::A => Keycode::A, G::B => Keycode::B, G::C => Keycode::C, G::D => Keycode::D,
            G::E => Keycode::E, G::F => Keycode::F, G::G => Keycode::G, G::H => Keycode::H,
            G::I => Keycode::I, G::J => Keycode::J, G::K => Keycode::K, G::L => Keycode::L,
            G::M => Keycode::M, G::N => Keycode::N, G::O => Keycode::O, G::P => Keycode::P,
            G::Q => Keycode::Q, G::R => Keycode::R, G::S => Keycode::S, G::T => Keycode::T,
            G::U => Keycode::U, G::V => Keycode::V, G::W => Keycode::W, G::X => Keycode::X,
            G::Y => Keycode::Y, G::Z => Keycode::Z,
            G::Num0 => Keycode::Num0, G::Num1 => Keycode::Num1, G::Num2 => Keycode::Num2,
            G::Num3 => Keycode::Num3, G::Num4 => Keycode::Num4, G::Num5 => Keycode::Num5,
            G::Num6 => Keycode::Num6, G::Num7 => Keycode::Num7, G::Num8 => Keycode::Num8,
            G::Num9 => Keycode::Num9,
            G::F1 => Keycode::F1, G::F2 => Keycode::F2, G::F3 => Keycode::F3, G::F4 => Keycode::F4,
            G::F5 => Keycode::F5, G::F6 => Keycode::F6, G::F7 => Keycode::F7, G::F8 => Keycode::F8,
            G::F9 => Keycode::F9, G::F10 => Keycode::F10, G::F11 => Keycode::F11, G::F12 => Keycode::F12,
            G::Space => Keycode::Space, G::Tab => Keycode::Tab, G::Enter => Keycode::Enter,
            G::Backspace => Keycode::Backspace, G::Escape => Keycode::Escape,
            G::Comma => Keycode::Comma, G::Period => Keycode::Period, G::Slash => Keycode::Slash,
            G::Semicolon => Keycode::Semicolon, G::Apostrophe => Keycode::Apostrophe,
            G::Minus => Keycode::Minus, G::Equal => Keycode::Equal,
            G::LeftBracket => Keycode::LeftBracket, G::RightBracket => Keycode::RightBracket,
            G::Backslash => Keycode::Backslash, G::GraveAccent => Keycode::GraveAccent,
            G::LeftShift => Keycode::LeftShift, G::RightShift => Keycode::RightShift,
            G::LeftControl => Keycode::LeftControl, G::RightControl => Keycode::RightControl,
            G::LeftAlt => Keycode::LeftAlt, G::RightAlt => Keycode::RightAlt,
            G::LeftSuper => Keycode::LeftSuper, G::RightSuper => Keycode::RightSuper,
            G::CapsLock => Keycode::CapsLock, G::NumLock => Keycode::NumLock,
            G::ScrollLock => Keycode::ScrollLock,
            G::Up => Keycode::Up, G::Down => Keycode::Down, G::Left => Keycode::Left,
            G::Right => Keycode::Right,
            G::Insert => Keycode::Insert, G::Delete => Keycode::Delete, G::Home => Keycode::Home,
            G::End => Keycode::End, G::PageUp => Keycode::PageUp, G::PageDown => Keycode::PageDown,
            _ => return None,
        })
    }

    /// Translate a `glfw::WindowEvent` into our neutral event (or `None` for
    /// event kinds the engine ignores).
    pub fn input_event_from_glfw(ev: &glfw::WindowEvent) -> Option<InputEvent> {
        Some(match *ev {
            glfw::WindowEvent::Key(key, _scancode, action, _mods) => match keycode_from_glfw(key) {
                Some(code) => InputEvent::Key { code, action: action.into() },
                None => InputEvent::UnknownKey { action: action.into() },
            },
            glfw::WindowEvent::MouseButton(button, action, _mods) =>
                InputEvent::MouseButton { button: button.into(), action: action.into() },
            glfw::WindowEvent::CursorPos(x, y) => InputEvent::CursorPos { x, y },
            glfw::WindowEvent::Scroll(x, y) => InputEvent::Scroll { x, y },
            _ => return None,
        })
    }
}
#[cfg(feature = "glfw")]
pub use glfw_conv::{input_event_from_glfw, keycode_from_glfw};

// =============================================================================
// winit -> neutral (new backend). Gated on the winit dep.
// =============================================================================
#[cfg(feature = "winit")]
mod winit_conv {
    use super::*;
    use winit::event::{ElementState, MouseButton as WMouse};
    use winit::keyboard::{KeyCode as WKey, PhysicalKey};

    /// winit reports press/release; auto-repeat rides a `repeat: bool` flag on
    /// the `KeyEvent`, so the caller passes it in to recover our `Repeat`.
    pub fn key_action(state: ElementState, repeat: bool) -> KeyAction {
        match state {
            ElementState::Pressed if repeat => KeyAction::Repeat,
            ElementState::Pressed => KeyAction::Press,
            ElementState::Released => KeyAction::Release,
        }
    }

    impl From<WMouse> for MouseBtn {
        fn from(b: WMouse) -> Self {
            match b {
                WMouse::Left => MouseBtn::Left,
                WMouse::Right => MouseBtn::Right,
                WMouse::Middle => MouseBtn::Middle,
                WMouse::Back => MouseBtn::Back,
                WMouse::Forward => MouseBtn::Forward,
                WMouse::Other(n) => MouseBtn::Other(n),
            }
        }
    }

    /// Map a winit physical `KeyCode` to our neutral keycode (physical layout,
    /// matching glfw's physical-key semantics). `None` for unmodelled keys.
    pub fn keycode_from_winit(pk: PhysicalKey) -> Option<Keycode> {
        let k = match pk {
            PhysicalKey::Code(c) => c,
            PhysicalKey::Unidentified(_) => return None,
        };
        Some(match k {
            WKey::KeyA => Keycode::A, WKey::KeyB => Keycode::B, WKey::KeyC => Keycode::C,
            WKey::KeyD => Keycode::D, WKey::KeyE => Keycode::E, WKey::KeyF => Keycode::F,
            WKey::KeyG => Keycode::G, WKey::KeyH => Keycode::H, WKey::KeyI => Keycode::I,
            WKey::KeyJ => Keycode::J, WKey::KeyK => Keycode::K, WKey::KeyL => Keycode::L,
            WKey::KeyM => Keycode::M, WKey::KeyN => Keycode::N, WKey::KeyO => Keycode::O,
            WKey::KeyP => Keycode::P, WKey::KeyQ => Keycode::Q, WKey::KeyR => Keycode::R,
            WKey::KeyS => Keycode::S, WKey::KeyT => Keycode::T, WKey::KeyU => Keycode::U,
            WKey::KeyV => Keycode::V, WKey::KeyW => Keycode::W, WKey::KeyX => Keycode::X,
            WKey::KeyY => Keycode::Y, WKey::KeyZ => Keycode::Z,
            WKey::Digit0 => Keycode::Num0, WKey::Digit1 => Keycode::Num1, WKey::Digit2 => Keycode::Num2,
            WKey::Digit3 => Keycode::Num3, WKey::Digit4 => Keycode::Num4, WKey::Digit5 => Keycode::Num5,
            WKey::Digit6 => Keycode::Num6, WKey::Digit7 => Keycode::Num7, WKey::Digit8 => Keycode::Num8,
            WKey::Digit9 => Keycode::Num9,
            WKey::F1 => Keycode::F1, WKey::F2 => Keycode::F2, WKey::F3 => Keycode::F3,
            WKey::F4 => Keycode::F4, WKey::F5 => Keycode::F5, WKey::F6 => Keycode::F6,
            WKey::F7 => Keycode::F7, WKey::F8 => Keycode::F8, WKey::F9 => Keycode::F9,
            WKey::F10 => Keycode::F10, WKey::F11 => Keycode::F11, WKey::F12 => Keycode::F12,
            WKey::Space => Keycode::Space, WKey::Tab => Keycode::Tab,
            WKey::Enter => Keycode::Enter, WKey::Backspace => Keycode::Backspace,
            WKey::Escape => Keycode::Escape,
            WKey::Comma => Keycode::Comma, WKey::Period => Keycode::Period, WKey::Slash => Keycode::Slash,
            WKey::Semicolon => Keycode::Semicolon, WKey::Quote => Keycode::Apostrophe,
            WKey::Minus => Keycode::Minus, WKey::Equal => Keycode::Equal,
            WKey::BracketLeft => Keycode::LeftBracket, WKey::BracketRight => Keycode::RightBracket,
            WKey::Backslash => Keycode::Backslash, WKey::Backquote => Keycode::GraveAccent,
            WKey::ShiftLeft => Keycode::LeftShift, WKey::ShiftRight => Keycode::RightShift,
            WKey::ControlLeft => Keycode::LeftControl, WKey::ControlRight => Keycode::RightControl,
            WKey::AltLeft => Keycode::LeftAlt, WKey::AltRight => Keycode::RightAlt,
            WKey::SuperLeft => Keycode::LeftSuper, WKey::SuperRight => Keycode::RightSuper,
            WKey::CapsLock => Keycode::CapsLock, WKey::NumLock => Keycode::NumLock,
            WKey::ScrollLock => Keycode::ScrollLock,
            WKey::ArrowUp => Keycode::Up, WKey::ArrowDown => Keycode::Down,
            WKey::ArrowLeft => Keycode::Left, WKey::ArrowRight => Keycode::Right,
            WKey::Insert => Keycode::Insert, WKey::Delete => Keycode::Delete,
            WKey::Home => Keycode::Home, WKey::End => Keycode::End,
            WKey::PageUp => Keycode::PageUp, WKey::PageDown => Keycode::PageDown,
            _ => return None,
        })
    }
}
#[cfg(feature = "winit")]
pub use winit_conv::{key_action as winit_key_action, keycode_from_winit};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_round_trips_for_every_keycode() {
        for &k in Keycode::ALL {
            assert_eq!(Keycode::from_name(k.name()), Some(k), "name round-trip failed for {:?}", k);
        }
    }

    #[test]
    fn i32_round_trips_and_is_dense() {
        for (i, &k) in Keycode::ALL.iter().enumerate() {
            assert_eq!(k.to_i32(), i as i32);
            assert_eq!(Keycode::from_i32(i as i32), Some(k));
        }
        assert_eq!(Keycode::from_i32(-1), None);
        assert_eq!(Keycode::from_i32(Keycode::ALL.len() as i32), None);
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<&str> = Keycode::ALL.iter().map(|k| k.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "duplicate Keycode name detected");
    }

    #[test]
    fn known_glfw_compatible_names_present() {
        // These exact strings appear in shipped keybindings.json files; they
        // must keep resolving so saved binds survive the backend swap.
        for n in ["Space", "LeftShift", "LeftControl", "Period", "Semicolon",
                  "Num1", "F3", "Escape", "GraveAccent", "PageDown"] {
            assert!(Keycode::from_name(n).is_some(), "missing compat key name {:?}", n);
        }
    }

    #[test]
    fn unknown_name_is_none() {
        assert_eq!(Keycode::from_name("Menu"), None);
        assert_eq!(Keycode::from_name(""), None);
    }
}
