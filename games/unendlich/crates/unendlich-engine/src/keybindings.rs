//! Configurable keybindings.
//!
//! Actions map to native `Keycode`s (input_codes). Bindings load/save as JSON. Because `glfw::Key`
//! does not implement `Serialize`/`Deserialize`, we serialize the integer
//! scancode (`Key as i32`) and reconstruct via `Key::from_i32` on load.

// r328: keybindings is backend-neutral — keys/buttons are native
// input_codes types, so this compiles & unit-tests with no glfw.
use crate::input_codes::{Keycode as Key, MouseBtn as MouseButton};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GameAction {
    // Movement
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    Jump,
    Sneak,
    Sprint,

    // Flight
    FlyUp,
    FlyDown,
    ToggleFlight,

    // Inventory
    OpenInventory,
    DropItem,
    Hotbar1,
    Hotbar2,
    Hotbar3,
    Hotbar4,
    Hotbar5,
    Hotbar6,
    Hotbar7,
    Hotbar8,
    Hotbar9,

    // UI / debug
    ToggleDebug,
    ToggleDebugOverlay,
    /// Decrease tick speed multiplier (F6). Geometric step (sqrt(10)),
    /// so 6 presses = 1000x slower. Used for showcasing slow simulation
    /// features in fast-forward.
    TickSpeedSlower,
    /// Increase tick speed multiplier (F7).
    TickSpeedFaster,
    Screenshot,
    ToggleFullscreen,
    ToggleChunkBorders,
    ReloadChunks,
    /// r515: toggle first-person ↔ third-person self-view.
    ToggleThirdPerson,
    /// r515: toggle the cinematic over-the-shoulder camera.
    ToggleCinematicCam,
    /// r528: hold to channel a bending cast (elemental discharge). Rebindable so
    /// it works across keyboard layouts — physical `B` is wrong on Dvorak, which
    /// was why "B does nothing" for the operator.
    Bend,
    /// Spawn a heat source (fire) at the player's current position.
    /// Demo / playtest: drives the temperature → convection coupling
    /// so the F4 wind arrow visibly tilts up where fires were placed.
    /// Will be replaced by proper inventory + flint-and-steel later.
    LightFire,
}

#[derive(Debug, Clone)]
pub struct KeyBindings {
    /// Map from action to a list of physical keys that all trigger it.
    /// Multiple keys per action gives us defensive overlap (e.g. both
    /// `;` and `LeftShift` for crouch/fly-down) so the action works
    /// across keyboard layouts and user expectations.
    bindings: HashMap<GameAction, Vec<Key>>,
    /// Map from action to mouse buttons. Same multi-binding shape as
    /// keyboard. Used for actions that are naturally mouse-driven
    /// (place block, pick block, etc.) and also for "whichever feels
    /// right" combinations (e.g. middle-click could trigger LightFire
    /// in addition to F9).
    ///
    /// Note: left and right click have HARDCODED meaning in the bin's
    /// loop today (break/place via mouse_left_held / mouse_right_held).
    /// Routing those through this binding system is a future cleanup;
    /// for now this map covers buttons that don't conflict with the
    /// hardcoded usage (Button3/Middle, Button4, Button5 typically).
    mouse_bindings: HashMap<GameAction, Vec<MouseButton>>,
}

/// Disk format. We persist the action -> keycode list. Backwards-compat
/// loader handles the older single-key form by wrapping it in a Vec.
#[derive(Serialize, Deserialize)]
struct StoredBindings {
    bindings: HashMap<GameAction, Vec<String>>,
    /// r528: mouse-button bindings, serialized as MouseBtn Debug names
    /// ("Back", "Forward", "Middle", …). `default` keeps old files loadable.
    #[serde(default)]
    mouse: HashMap<GameAction, Vec<String>>,
}

/// r528: parse a MouseBtn from its Debug name (round-trips the side buttons a
/// player would actually bind). `Other(n)` is not persisted.
fn mouse_from_name(name: &str) -> Option<MouseButton> {
    match name {
        "Left" => Some(MouseButton::Left),
        "Right" => Some(MouseButton::Right),
        "Middle" => Some(MouseButton::Middle),
        "Back" => Some(MouseButton::Back),
        "Forward" => Some(MouseButton::Forward),
        _ => None,
    }
}

impl Default for KeyBindings {
    fn default() -> Self { Self::dvorak_preset() }
}

impl KeyBindings {
    pub fn from_map(bindings: HashMap<GameAction, Vec<Key>>) -> Self {
        Self { bindings, mouse_bindings: HashMap::new() }
    }

    /// Helper used by preset constructors: convert a single-key map into
    /// the multi-key form by wrapping each value in a Vec.
    fn from_single_key_map(b: HashMap<GameAction, Key>) -> Self {
        Self {
            bindings: b.into_iter()
                .map(|(a, k)| (a, vec![k]))
                .collect(),
            mouse_bindings: HashMap::new(),
        }
    }

    /// Add an extra key for an existing action. Used to give Sneak / FlyDown
    /// both `;` and `LeftShift` so it works regardless of which key the
    /// user reaches for.
    pub fn add_key(&mut self, action: GameAction, key: Key) {
        self.bindings.entry(action).or_default().push(key);
    }

    /// DVORAK preset — touch-typing position for the **right hand** (default).
    /// Fwd=`.`, back=`e`, left=`o`, right=`u` puts WASD-equivalent on the
    /// home row + top row. Sprint=`a` (left of home row, easy to hold with
    /// pinky), sneak=`;` (mirrored position on the right). Jump=Space.
    ///
    /// Left-handed layout (mirrored): use `dvorak_lefthand_preset()`.
    pub fn dvorak_preset() -> Self {
        let mut b = HashMap::new();
        b.insert(GameAction::MoveForward, Key::Period);
        b.insert(GameAction::MoveBackward, Key::E);
        b.insert(GameAction::MoveLeft, Key::O);
        b.insert(GameAction::MoveRight, Key::U);
        b.insert(GameAction::Jump, Key::Space);
        b.insert(GameAction::Sneak, Key::Semicolon);
        b.insert(GameAction::Sprint, Key::A);
        b.insert(GameAction::FlyUp, Key::Space);
        b.insert(GameAction::FlyDown, Key::Semicolon);
        b.insert(GameAction::ToggleFlight, Key::F);
        let mut kb = Self::with_common_bindings(b);
        // Defensive secondary bindings — ONLY for actions that can't
        // collide with movement / control keys in this preset:
        //   - LeftShift for Sneak/FlyDown is universal "down/crouch"; not
        //     used for movement in DVORAK so no collision.
        //   - LeftControl for Sprint is universal "fast"; same.
        // We do NOT add cross-preset ToggleFlight aliases — that would
        // collide with movement keys in OTHER presets (e.g. F is MoveRight
        // in QWERTY, so adding F as a ToggleFlight secondary in QWERTY
        // would toggle flight every time the player moves right).
        kb.add_key(GameAction::Sneak, Key::LeftShift);
        kb.add_key(GameAction::FlyDown, Key::LeftShift);
        kb.add_key(GameAction::Sprint, Key::LeftControl);
        // r202: ARROW KEYS as universal movement fallback. Operator reported
        // Dvorak letter-position confusion makes play difficult — the arrows
        // are layout-independent (same physical keys on every layout), so
        // movement ALWAYS works even if the letter cluster feels wrong. They
        // can't collide with letter movement and are not used for anything
        // else in this preset.
        kb.add_key(GameAction::MoveForward, Key::Up);
        kb.add_key(GameAction::MoveBackward, Key::Down);
        kb.add_key(GameAction::MoveLeft, Key::Left);
        kb.add_key(GameAction::MoveRight, Key::Right);
        // r528: on Dvorak the key LABELED 'b' is at the physical QWERTY-N
        // position — bind Bend there too so the operator's 'b' actually casts.
        kb.add_key(GameAction::Bend, Key::N);
        kb
    }

    /// QWERTY preset — touch-typing position for the **left hand** (default).

    /// Fwd=`e`, back=`d`, left=`s`, right=`f` puts movement on home row + top
    /// row, hand naturally resting. Sprint=`a` (pinky on home row),
    /// sneak=`;` (right pinky home position).
    pub fn qwerty_preset() -> Self {
        let mut b = HashMap::new();
        b.insert(GameAction::MoveForward, Key::E);
        b.insert(GameAction::MoveBackward, Key::D);
        b.insert(GameAction::MoveLeft, Key::S);
        b.insert(GameAction::MoveRight, Key::F);
        b.insert(GameAction::Jump, Key::Space);
        b.insert(GameAction::Sneak, Key::Semicolon);
        b.insert(GameAction::Sprint, Key::A);
        b.insert(GameAction::FlyUp, Key::Space);
        b.insert(GameAction::FlyDown, Key::Semicolon);
        b.insert(GameAction::ToggleFlight, Key::G);
        let mut kb = Self::with_common_bindings(b);
        kb.add_key(GameAction::Sneak, Key::LeftShift);
        kb.add_key(GameAction::FlyDown, Key::LeftShift);
        kb.add_key(GameAction::Sprint, Key::LeftControl);
        // r202: arrow keys as a layout-independent movement fallback
        // (matches the DVORAK preset, so movement is consistent across
        // presets and never depends on letter position).
        kb.add_key(GameAction::MoveForward, Key::Up);
        kb.add_key(GameAction::MoveBackward, Key::Down);
        kb.add_key(GameAction::MoveLeft, Key::Left);
        kb.add_key(GameAction::MoveRight, Key::Right);
        // NOTE: do NOT alias ToggleFlight to F here — F is MoveRight in
        // this preset. Cross-preset aliases caused a critical bug where
        // moving right would toggle flight every key press.
        kb
    }

    /// Classic WASD QWERTY preset for players accustomed to that layout.
    /// (Kept around as an option; not the default.)
    pub fn qwerty_wasd_preset() -> Self {
        let mut b = HashMap::new();
        b.insert(GameAction::MoveForward, Key::W);
        b.insert(GameAction::MoveBackward, Key::S);
        b.insert(GameAction::MoveLeft, Key::A);
        b.insert(GameAction::MoveRight, Key::D);
        b.insert(GameAction::Jump, Key::Space);
        b.insert(GameAction::Sneak, Key::LeftShift);
        b.insert(GameAction::Sprint, Key::LeftControl);
        b.insert(GameAction::FlyUp, Key::Space);
        b.insert(GameAction::FlyDown, Key::LeftShift);
        b.insert(GameAction::ToggleFlight, Key::F);
        let mut kb = Self::with_common_bindings(b);
        // Semicolon as secondary for Sneak/FlyDown is fine — not used for
        // anything in WASD.
        kb.add_key(GameAction::Sneak, Key::Semicolon);
        kb.add_key(GameAction::FlyDown, Key::Semicolon);
        kb
    }

    fn with_common_bindings(mut b: HashMap<GameAction, Key>) -> Self {
        // Inventory/panel on the physical Dvorak `p` key. GLFW names keys by US
        // physical position, so the key that types `p` in a Dvorak OS layout is
        // the one a US board prints `R` on -> Key::R. `I` kept as a secondary.
        b.insert(GameAction::OpenInventory, Key::R);
        b.insert(GameAction::DropItem, Key::Q);
        b.insert(GameAction::Hotbar1, Key::Num1);
        b.insert(GameAction::Hotbar2, Key::Num2);
        b.insert(GameAction::Hotbar3, Key::Num3);
        b.insert(GameAction::Hotbar4, Key::Num4);
        b.insert(GameAction::Hotbar5, Key::Num5);
        b.insert(GameAction::Hotbar6, Key::Num6);
        b.insert(GameAction::Hotbar7, Key::Num7);
        b.insert(GameAction::Hotbar8, Key::Num8);
        b.insert(GameAction::Hotbar9, Key::Num9);
        b.insert(GameAction::ToggleDebug, Key::F3);
        b.insert(GameAction::ToggleDebugOverlay, Key::F4);
        b.insert(GameAction::ReloadChunks, Key::F5);
        b.insert(GameAction::TickSpeedSlower, Key::F6);
        b.insert(GameAction::TickSpeedFaster, Key::F7);
        b.insert(GameAction::ToggleChunkBorders, Key::F8);
        b.insert(GameAction::LightFire, Key::F9);
        b.insert(GameAction::Screenshot, Key::F2);
        b.insert(GameAction::ToggleFullscreen, Key::F11);
        // r515: camera views — third-person (F5) and cinematic shoulder (Y). Rebindable.
        b.insert(GameAction::ToggleThirdPerson, Key::F5);
        b.insert(GameAction::ToggleCinematicCam, Key::Y);
        b.insert(GameAction::Bend, Key::B); // r528: rebindable bending cast (hold)
        let mut kb = Self::from_single_key_map(b);
        // `I` stays as a secondary alias for the inventory (P is primary).
        kb.add_key(GameAction::OpenInventory, Key::I);
        kb
    }

    /// Returns the FIRST key bound to this action (legacy single-key API).
    /// Prefer `keys_for_action` when you need all of them.
    pub fn get_key(&self, action: GameAction) -> Option<Key> {
        self.bindings.get(&action).and_then(|v| v.first().copied())
    }

    /// All keys bound to an action. Used by `is_action_pressed` to check
    /// every binding.
    pub fn keys_for_action(&self, action: GameAction) -> &[Key] {
        self.bindings.get(&action).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Returns *some* action bound to this key. Multiple actions may share a
    /// key (e.g. Jump and FlyUp both on Space) — in that case any one of them
    /// may be returned. Used mostly for the rebind UI; gameplay code should
    /// use `is_action_pressed`.
    pub fn get_action(&self, key: Key) -> Option<GameAction> {
        self.bindings.iter()
            .find(|(_, ks)| ks.iter().any(|k| *k == key))
            .map(|(a, _)| *a)
    }

    /// All actions bound to a given key. Useful for showing rebind conflicts.
    pub fn actions_for_key(&self, key: Key) -> Vec<GameAction> {
        self.bindings.iter()
            .filter(|(_, ks)| ks.iter().any(|k| *k == key))
            .map(|(a, _)| *a)
            .collect()
    }

    /// Bind `action` to a single `key`, clearing all of action's prior
    /// bindings. (Use `add_key` to ADD without clearing.)
    pub fn set_binding(&mut self, action: GameAction, key: Key) {
        // Clear this key from OTHER actions, EXCEPT overlap-allowed pairs (r319):
        // FlyUp/Jump and FlyDown/Sneak share a key on purpose so Space / `;` mean
        // up / down in both physics and fly modes.
        for (a, ks) in self.bindings.iter_mut() {
            if *a != action && !Self::overlap_allowed(action, *a) {
                ks.retain(|k| *k != key);
            }
        }
        // Replace action's bindings with just `key`.
        self.bindings.insert(action, vec![key]);
    }

    /// Whether two actions may share a key (rebinding one keeps the other).
    fn overlap_allowed(a: GameAction, b: GameAction) -> bool {
        use GameAction::*;
        let pair = |x, y| (a == x && b == y) || (a == y && b == x);
        pair(FlyUp, Jump) || pair(FlyDown, Sneak)
    }

    /// True if ANY key bound to this action is currently pressed.
    pub fn is_action_pressed(&self, action: GameAction, input: &crate::input::InputCache) -> bool {
        self.keys_for_action(action).iter().any(|k| input.is_key_pressed(*k))
    }

    /// True if ANY key bound to this action transitioned from up to down
    /// this frame. Use for non-repeating actions (jump, toggle, place block).
    pub fn is_action_just_pressed(&self, action: GameAction, input: &crate::input::InputCache) -> bool {
        self.keys_for_action(action).iter().any(|k| input.is_key_just_pressed(*k))
    }

    /// Mouse-button bindings for this action. Used by callers that
    /// route mouse buttons through the binding system instead of
    /// hardcoding `MouseButton::Button1` etc.
    pub fn mouse_buttons_for_action(&self, action: GameAction) -> &[MouseButton] {
        self.mouse_bindings.get(&action).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Bind a mouse button to an action. Multiple buttons can be
    /// bound to the same action (e.g., middle-click AND a side button
    /// could both trigger LightFire). Idempotent: re-binding the same
    /// (action, button) is a no-op.
    pub fn bind_mouse(&mut self, action: GameAction, button: MouseButton) {
        let buttons = self.mouse_bindings.entry(action).or_insert_with(Vec::new);
        if !buttons.contains(&button) {
            buttons.push(button);
        }
    }

    /// r528: REPLACE `action`'s binding with a single mouse `button` — a clean
    /// rebind from the Controls page. Clears the action's keyboard keys and frees
    /// the button from any other action, so the row reads as just that button.
    /// (This is what lets a player put e.g. Bending on a side mouse button.)
    pub fn set_mouse_binding(&mut self, action: GameAction, button: MouseButton) {
        for (a, bs) in self.mouse_bindings.iter_mut() {
            if *a != action {
                bs.retain(|b| *b != button);
            }
        }
        self.bindings.remove(&action);
        self.mouse_bindings.insert(action, vec![button]);
    }

    /// True if ANY mouse button bound to this action is currently held.
    pub fn is_action_pressed_mouse(&self, action: GameAction, input: &crate::input::InputCache) -> bool {
        self.mouse_buttons_for_action(action).iter()
            .any(|b| input.is_mouse_button_pressed(*b))
    }

    /// True if ANY mouse button bound to this action transitioned from
    /// up to down this frame. Use for non-repeating mouse-driven
    /// actions (e.g. middle-click for pick-block, side-button for
    /// hotbar prev/next).
    pub fn is_action_just_pressed_mouse(&self, action: GameAction, input: &crate::input::InputCache) -> bool {
        self.mouse_buttons_for_action(action).iter()
            .any(|b| input.is_mouse_button_just_pressed(*b))
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        // Serialize keys as their Debug names ("Space", "Comma", etc.).
        // Stable across glfw versions and easy to edit by hand.
        let stored = StoredBindings {
            bindings: self.bindings.iter()
                .map(|(a, ks)| (*a, ks.iter().map(|k| format!("{:?}", k)).collect()))
                .collect(),
            mouse: self.mouse_bindings.iter()
                .map(|(a, bs)| (*a, bs.iter().map(|b| format!("{:?}", b)).collect()))
                .collect(),
        };
        let json = serde_json::to_string_pretty(&stored)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, json)
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        let json = fs::read_to_string(path)?;
        let stored: StoredBindings = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let mut bindings: HashMap<GameAction, Vec<Key>> = HashMap::new();
        for (action, names) in stored.bindings {
            let mut keys = Vec::new();
            for name in names {
                if let Some(key) = key_from_name(&name) {
                    keys.push(key);
                } else {
                    log::warn!("Discarding unknown key {:?} for action {:?}", name, action);
                }
            }
            if !keys.is_empty() {
                bindings.insert(action, keys);
            }
        }
        let mut kb = Self::from_map(bindings);
        // r528: restore mouse-button bindings.
        for (action, names) in stored.mouse {
            for name in names {
                if let Some(btn) = mouse_from_name(&name) {
                    kb.bind_mouse(action, btn);
                } else {
                    log::warn!("Discarding unknown mouse button {:?} for {:?}", name, action);
                }
            }
        }
        Ok(kb)
    }

    pub fn load_or_default<P: AsRef<Path>>(path: P) -> Self {
        Self::load_from_file(path).unwrap_or_default()
    }
}

/// Convert a key's `Debug`/`name()` string back to the native `Keycode`.
///
/// Generated by listing all variants used in the codebase. If a binding refers
/// to a key not in this table it is dropped at load with a warning, and the
/// default for that action is used instead.
fn key_from_name(name: &str) -> Option<Key> {
    Some(match name {
        // Letters
        "A" => Key::A, "B" => Key::B, "C" => Key::C, "D" => Key::D,
        "E" => Key::E, "F" => Key::F, "G" => Key::G, "H" => Key::H,
        "I" => Key::I, "J" => Key::J, "K" => Key::K, "L" => Key::L,
        "M" => Key::M, "N" => Key::N, "O" => Key::O, "P" => Key::P,
        "Q" => Key::Q, "R" => Key::R, "S" => Key::S, "T" => Key::T,
        "U" => Key::U, "V" => Key::V, "W" => Key::W, "X" => Key::X,
        "Y" => Key::Y, "Z" => Key::Z,

        // Number row
        "Num0" => Key::Num0, "Num1" => Key::Num1, "Num2" => Key::Num2,
        "Num3" => Key::Num3, "Num4" => Key::Num4, "Num5" => Key::Num5,
        "Num6" => Key::Num6, "Num7" => Key::Num7, "Num8" => Key::Num8,
        "Num9" => Key::Num9,

        // Function keys
        "F1" => Key::F1, "F2" => Key::F2, "F3" => Key::F3, "F4" => Key::F4,
        "F5" => Key::F5, "F6" => Key::F6, "F7" => Key::F7, "F8" => Key::F8,
        "F9" => Key::F9, "F10" => Key::F10, "F11" => Key::F11, "F12" => Key::F12,

        // Whitespace / punctuation
        "Space" => Key::Space, "Tab" => Key::Tab, "Enter" => Key::Enter,
        "Backspace" => Key::Backspace, "Escape" => Key::Escape,
        "Comma" => Key::Comma, "Period" => Key::Period, "Slash" => Key::Slash,
        "Semicolon" => Key::Semicolon, "Apostrophe" => Key::Apostrophe,
        "Minus" => Key::Minus, "Equal" => Key::Equal,
        "LeftBracket" => Key::LeftBracket, "RightBracket" => Key::RightBracket,
        "Backslash" => Key::Backslash, "GraveAccent" => Key::GraveAccent,

        // Modifiers
        "LeftShift" => Key::LeftShift, "RightShift" => Key::RightShift,
        "LeftControl" => Key::LeftControl, "RightControl" => Key::RightControl,
        "LeftAlt" => Key::LeftAlt, "RightAlt" => Key::RightAlt,
        "LeftSuper" => Key::LeftSuper, "RightSuper" => Key::RightSuper,
        "CapsLock" => Key::CapsLock, "NumLock" => Key::NumLock,
        "ScrollLock" => Key::ScrollLock,

        // Arrows + nav
        "Up" => Key::Up, "Down" => Key::Down, "Left" => Key::Left, "Right" => Key::Right,
        "Insert" => Key::Insert, "Delete" => Key::Delete,
        "Home" => Key::Home, "End" => Key::End,
        "PageUp" => Key::PageUp, "PageDown" => Key::PageDown,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_have_movement_keys() {
        for preset in [KeyBindings::dvorak_preset(), KeyBindings::qwerty_preset()] {
            assert!(preset.get_key(GameAction::MoveForward).is_some());
            assert!(preset.get_key(GameAction::MoveBackward).is_some());
            assert!(preset.get_key(GameAction::MoveLeft).is_some());
            assert!(preset.get_key(GameAction::MoveRight).is_some());
            assert!(preset.get_key(GameAction::Jump).is_some());
        }
    }

    #[test]
    fn rebind_replaces_old() {
        let mut kb = KeyBindings::dvorak_preset();
        kb.set_binding(GameAction::MoveForward, Key::W);
        assert_eq!(kb.get_key(GameAction::MoveForward), Some(Key::W));
        assert_ne!(kb.get_action(Key::Comma), Some(GameAction::MoveForward));
    }

    #[test]
    fn rebind_clears_conflict() {
        let mut kb = KeyBindings::dvorak_preset();
        kb.set_binding(GameAction::Sneak, Key::Space);
        assert_eq!(kb.get_action(Key::Space), Some(GameAction::Sneak));
        assert_ne!(kb.get_key(GameAction::Jump), Some(Key::Space));
    }

    #[test]
    fn round_trip_through_disk() {
        let kb = KeyBindings::qwerty_preset();
        let path = std::env::temp_dir().join("unendlich_kb_test.json");
        kb.save_to_file(&path).unwrap();
        let kb2 = KeyBindings::load_from_file(&path).unwrap();
        assert_eq!(kb.get_key(GameAction::MoveForward), kb2.get_key(GameAction::MoveForward));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn toggle_flight_does_not_collide_with_movement() {
        // CRITICAL invariant: ToggleFlight must not share any key with the
        // movement actions (Move*, Jump, Sneak, Sprint). Otherwise pressing
        // a movement key would also toggle flight — which broke physics
        // mode badly in an earlier version.
        for preset in [
            KeyBindings::dvorak_preset(),
            KeyBindings::qwerty_preset(),
            KeyBindings::qwerty_wasd_preset(),
        ] {
            let toggle_keys: Vec<Key> = preset.keys_for_action(GameAction::ToggleFlight)
                .iter().copied().collect();
            for action in &[
                GameAction::MoveForward, GameAction::MoveBackward,
                GameAction::MoveLeft, GameAction::MoveRight,
                GameAction::Jump, GameAction::Sneak, GameAction::Sprint,
                GameAction::FlyUp, GameAction::FlyDown,
            ] {
                for k in preset.keys_for_action(*action) {
                    assert!(!toggle_keys.contains(k),
                        "ToggleFlight collides with {:?} on key {:?} — would toggle on movement!",
                        action, k);
                }
            }
        }
    }

    /// Mouse bindings start empty (presets don't bind any mouse buttons today).
    #[test]
    fn mouse_bindings_start_empty() {
        let kb = KeyBindings::dvorak_preset();
        assert!(kb.mouse_buttons_for_action(GameAction::LightFire).is_empty());
        assert!(kb.mouse_buttons_for_action(GameAction::Jump).is_empty());
    }

    /// Binding a mouse button shows up in the lookup.
    #[test]
    fn bind_mouse_appears_in_lookup() {
        let mut kb = KeyBindings::dvorak_preset();
        kb.bind_mouse(GameAction::LightFire, MouseButton::Middle);
        let buttons = kb.mouse_buttons_for_action(GameAction::LightFire);
        assert_eq!(buttons.len(), 1);
        assert_eq!(buttons[0], MouseButton::Middle);
    }

    /// Re-binding the same (action, button) is a no-op (idempotent).
    #[test]
    fn bind_mouse_is_idempotent() {
        let mut kb = KeyBindings::dvorak_preset();
        kb.bind_mouse(GameAction::LightFire, MouseButton::Middle);
        kb.bind_mouse(GameAction::LightFire, MouseButton::Middle);
        assert_eq!(kb.mouse_buttons_for_action(GameAction::LightFire).len(), 1);
    }

    /// Multiple buttons can be bound to one action (overlap support).
    #[test]
    fn multiple_buttons_per_action() {
        let mut kb = KeyBindings::dvorak_preset();
        kb.bind_mouse(GameAction::LightFire, MouseButton::Middle);
        kb.bind_mouse(GameAction::LightFire, MouseButton::Back);
        let buttons = kb.mouse_buttons_for_action(GameAction::LightFire);
        assert_eq!(buttons.len(), 2);
        assert!(buttons.contains(&MouseButton::Middle));
        assert!(buttons.contains(&MouseButton::Back));
    }
}
