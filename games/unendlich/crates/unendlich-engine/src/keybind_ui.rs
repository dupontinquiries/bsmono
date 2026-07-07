//! keybind_ui (r199) — presentation + capture helpers for the hotkey
//! rebinding page. Kept here (not in the bin) so it is unit-testable and
//! so the action ordering / labels live next to the GameAction enum.
//!
//! The Controls settings tab shows one row per [`rebindable_actions`]
//! entry: a label and the current key (via [`key_label`]). Clicking a row
//! puts the bin into capture mode; the bin scans [`capture_candidates`]
//! for the first just-pressed key and calls `KeyBindings::set_binding`.

use crate::keybindings::GameAction;
use crate::input_codes::Keycode as Key; // r328: native keycodes
use crate::input_codes::MouseBtn as MouseButton; // r528: mouse-button rebinding

/// The actions exposed on the rebinding page, in display order. Movement
/// and the common toggles first, hotbar last. Not every GameAction is
/// here — only the ones a player reasonably rebinds.
pub fn rebindable_actions() -> &'static [(GameAction, &'static str)] {
    use GameAction::*;
    &[
        (MoveForward, "Move Forward"),
        (MoveBackward, "Move Backward"),
        (MoveLeft, "Move Left"),
        (MoveRight, "Move Right"),
        (Jump, "Jump"),
        (Sneak, "Sneak"),
        (Sprint, "Sprint"),
        (FlyUp, "Fly Up"),
        (FlyDown, "Fly Down"),
        (ToggleFlight, "Toggle Flight"),
        (OpenInventory, "Open Inventory"),
        (DropItem, "Drop Item"),
        (Hotbar1, "Hotbar 1"),
        (Hotbar2, "Hotbar 2"),
        (Hotbar3, "Hotbar 3"),
        (Hotbar4, "Hotbar 4"),
        (Hotbar5, "Hotbar 5"),
        (ToggleDebugOverlay, "Toggle Debug Overlay"),
        (ToggleThirdPerson, "Third-Person View"),
        (ToggleCinematicCam, "Cinematic Camera"),
        (Bend, "Bending (hold to cast)"),
        (Screenshot, "Screenshot"),
        (ToggleFullscreen, "Toggle Fullscreen"),
    ]
}

/// Human-readable label for a glfw key. glfw's own names are mostly fine
/// (`Key::W` -> "W"), but spell out the ones whose Debug is ugly or
/// ambiguous so the page reads cleanly.
pub fn key_label(key: Key) -> String {
    match key {
        Key::Space => "Space".to_string(),
        Key::LeftShift => "L-Shift".to_string(),
        Key::RightShift => "R-Shift".to_string(),
        Key::LeftControl => "L-Ctrl".to_string(),
        Key::RightControl => "R-Ctrl".to_string(),
        Key::LeftAlt => "L-Alt".to_string(),
        Key::RightAlt => "R-Alt".to_string(),
        Key::Enter => "Enter".to_string(),
        Key::Tab => "Tab".to_string(),
        Key::Escape => "Esc".to_string(),
        Key::Up => "Up".to_string(),
        Key::Down => "Down".to_string(),
        Key::Left => "Left".to_string(),
        Key::Right => "Right".to_string(),
        // glfw Debug for letters/digits/F-keys is already clean: strip the
        // "Num" prefix glfw uses for the number row so "Num1" -> "1".
        other => {
            let d = format!("{other:?}");
            d.strip_prefix("Num").map(|s| s.to_string()).unwrap_or(d)
        }
    }
}

/// Candidate keys the capture loop scans for "the key the user just
/// pressed". Covers letters, digits, function keys, and the common
/// modifiers/navigation — the realistic bindable set. Escape is NOT here
/// (it cancels a pending rebind instead of binding).
pub fn capture_candidates() -> Vec<Key> {
    let mut v = Vec::new();
    // Letters A..Z
    const LETTERS: [Key; 26] = [
        Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H,
        Key::I, Key::J, Key::K, Key::L, Key::M, Key::N, Key::O, Key::P,
        Key::Q, Key::R, Key::S, Key::T, Key::U, Key::V, Key::W, Key::X,
        Key::Y, Key::Z,
    ];
    v.extend_from_slice(&LETTERS);
    const DIGITS: [Key; 10] = [
        Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4,
        Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
    ];
    v.extend_from_slice(&DIGITS);
    const FKEYS: [Key; 12] = [
        Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6,
        Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
    ];
    v.extend_from_slice(&FKEYS);
    v.extend_from_slice(&[
        Key::Space, Key::LeftShift, Key::RightShift, Key::LeftControl,
        Key::RightControl, Key::LeftAlt, Key::Tab, Key::Up, Key::Down,
        Key::Left, Key::Right,
        // r528: navigation cluster — players often map abilities to these (and
        // some mice emit PageUp/PageDown from wheel-tilt / side keys).
        Key::PageUp, Key::PageDown, Key::Home, Key::End, Key::Insert, Key::Delete,
    ]);
    v
}

/// r528: mouse buttons the capture loop offers for rebinding. Left/Right stay
/// reserved for look/place/break, so only the middle + the two side buttons
/// (`Back`/`Forward`, the "extra" buttons many mice expose) are bindable.
pub fn mouse_capture_candidates() -> Vec<MouseButton> {
    vec![MouseButton::Middle, MouseButton::Back, MouseButton::Forward]
}

/// Display label for a bound mouse button.
pub fn mouse_label(b: MouseButton) -> String {
    match b {
        MouseButton::Left => "Mouse-L".to_string(),
        MouseButton::Right => "Mouse-R".to_string(),
        MouseButton::Middle => "Mouse-Mid".to_string(),
        MouseButton::Back => "Mouse-4".to_string(),
        MouseButton::Forward => "Mouse-5".to_string(),
        MouseButton::Other(n) => format!("Mouse-{}", n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_clean() {
        assert_eq!(key_label(Key::W), "W");
        assert_eq!(key_label(Key::Num1), "1");
        assert_eq!(key_label(Key::Space), "Space");
        assert_eq!(key_label(Key::LeftShift), "L-Shift");
    }

    #[test]
    fn candidates_cover_and_exclude_escape() {
        let c = capture_candidates();
        assert!(c.contains(&Key::W));
        assert!(c.contains(&Key::Num5));
        assert!(c.contains(&Key::F3));
        assert!(!c.contains(&Key::Escape), "Esc must not be bindable (it cancels)");
    }

    #[test]
    fn action_list_nonempty_and_labeled() {
        let a = rebindable_actions();
        assert!(a.len() >= 15);
        assert!(a.iter().all(|(_, label)| !label.is_empty()));
        // movement keys present
        assert!(a.iter().any(|(act, _)| *act == GameAction::MoveForward));
    }
}
