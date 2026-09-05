//! The chat key router, lifted verbatim from the bin and run against a
//! stub of glfw's event types. The bug it replaces: the late drain
//! withheld chat keys from movement and then DROPPED them.
#![allow(dead_code)]
pub mod glfw {
    #[derive(Debug, Clone, Copy, PartialEq)] pub enum Action { Press, Release, Repeat }
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Key { Escape, Enter, KpEnter, Tab, Backspace, Up, Down, V, C, U, Z, P, W, A, S, D, Space }
    #[derive(Debug, Clone, Copy, PartialEq, Default)] pub struct Modifiers(u8);
    impl Modifiers {
        pub const Shift: Modifiers = Modifiers(1);
        pub const Control: Modifiers = Modifiers(2);
        pub fn contains(&self, m: Modifiers) -> bool { self.0 & m.0 != 0 }
        pub fn none() -> Self { Modifiers(0) }
    }
    #[derive(Debug, Clone, PartialEq)]
    pub enum WindowEvent { Char(char), Key(Key, i32, Action, Modifiers), Other }
    pub struct Window { pub clip: String }
    impl Window { pub fn get_clipboard_string(&self) -> Option<String> { Some(self.clip.clone()) } }
}

use glfw::*;
    fn route_chat_key(event: &WindowEvent, chat_focus: &mut bool,
                      chat_keys: &mut Vec<voxel_tools::chatbox::Key>,
                      chat_toggle: &mut bool, esc_pressed: &mut bool,
                      window: &Window) -> bool {
        
        use voxel_tools::chatbox::Key as CK;
        // Repeat counts as a press: holding Backspace must keep deleting,
        // and holding an arrow must keep moving through history.
        let pressed = |a: &Action| matches!(a, Action::Press | Action::Repeat);
        match event {
            WindowEvent::Char(ch) if *chat_focus => { chat_keys.push(CK::Char(*ch)); true }
            WindowEvent::Key(Key::Escape, _, a, _) if pressed(a) => {
                if *chat_focus { chat_keys.push(CK::Escape); *chat_focus = false; true }
                else { *esc_pressed = true; false }
            }
            WindowEvent::Key(Key::Enter, _, a, m) | WindowEvent::Key(Key::KpEnter, _, a, m)
                if pressed(a) =>
            {
                if *chat_focus {
                    chat_keys.push(if m.contains(glfw::Modifiers::Control) { CK::CtrlEnter }
                                   else { CK::Enter });
                    if !m.contains(glfw::Modifiers::Control) { *chat_focus = false; }
                } else {
                    *chat_toggle = true;
                    *chat_focus = true;
                }
                true
            }
            WindowEvent::Key(Key::Tab, _, a, m) if *chat_focus && pressed(a) => {
                chat_keys.push(if m.contains(glfw::Modifiers::Shift) { CK::ShiftTab } else { CK::Tab });
                true
            }
            WindowEvent::Key(Key::Backspace, _, a, _) if *chat_focus && pressed(a) => {
                chat_keys.push(CK::Backspace); true
            }
            WindowEvent::Key(Key::Up, _, a, _) if *chat_focus && pressed(a) => {
                chat_keys.push(CK::Up); true
            }
            WindowEvent::Key(Key::Down, _, a, _) if *chat_focus && pressed(a) => {
                chat_keys.push(CK::Down); true
            }
            WindowEvent::Key(Key::V, _, a, m) if pressed(a) && m.contains(glfw::Modifiers::Control) => {
                let text = window.get_clipboard_string().unwrap_or_default();
                if !text.is_empty() { chat_keys.push(CK::Paste(text)); }
                true
            }
            WindowEvent::Key(Key::C, _, a, m) if pressed(a) && m.contains(glfw::Modifiers::Control) => {
                chat_keys.push(if m.contains(glfw::Modifiers::Shift) { CK::CopyLog } else { CK::Copy });
                true
            }
            WindowEvent::Key(Key::U, _, a, m)
                if *chat_focus && pressed(a) && m.contains(glfw::Modifiers::Control) =>
            {
                chat_keys.push(CK::ClearLine); true
            }
            WindowEvent::Key(Key::Z, _, a, m) if pressed(a) && m.contains(glfw::Modifiers::Control) => {
                chat_keys.push(CK::CtrlZ); true
            }
            // Any other key while the chat has focus is swallowed here so
            // it cannot fall through to a hotkey arm.
            WindowEvent::Key(_, _, a, _) if *chat_focus && pressed(a) => true,
            _ => false,
        }
    }

#[cfg(test)]
mod tests {
    use super::*;
    use super::glfw::*;
    use voxel_tools::chatbox::Key as CK;
    fn press(k: Key) -> WindowEvent { WindowEvent::Key(k, 0, Action::Press, Modifiers::none()) }
    fn repeat(k: Key) -> WindowEvent { WindowEvent::Key(k, 0, Action::Repeat, Modifiers::none()) }
    fn run(events: &[WindowEvent], focus: bool) -> (Vec<CK>, bool, bool, Vec<bool>) {
        let (mut f, mut keys, mut tog, mut esc) = (focus, Vec::new(), false, false);
        let w = Window { clip: "pasted".into() };
        let consumed: Vec<bool> = events.iter()
            .map(|e| route_chat_key(e, &mut f, &mut keys, &mut tog, &mut esc, &w)).collect();
        (keys, f, tog, consumed)
    }
    #[test]
    fn enter_opens_and_claims_focus_for_the_rest_of_the_batch() {
        let (keys, focus, tog, consumed) = run(&[press(Key::Enter), WindowEvent::Char('h'),
                                                 press(Key::W), WindowEvent::Char('i')], false);
        assert!(tog && focus);
        assert_eq!(keys, vec![CK::Char('h'), CK::Char('i')]);
        assert!(consumed[2], "W is swallowed, not walked");
    }
    #[test]
    fn held_backspace_repeats() {
        let (keys, ..) = run(&[press(Key::Backspace), repeat(Key::Backspace), repeat(Key::Backspace)], true);
        assert_eq!(keys.len(), 3, "Repeat counts as a press");
    }
    #[test]
    fn escape_releases_focus_within_the_batch() {
        let (keys, focus, _, consumed) = run(&[press(Key::Escape), press(Key::W)], true);
        assert_eq!(keys, vec![CK::Escape]);
        assert!(!focus);
        assert!(!consumed[1], "after Esc, W goes back to the game");
    }
    #[test]
    fn nothing_with_focus_off_is_consumed_except_the_openers() {
        let (keys, _, _, consumed) = run(&[press(Key::W), WindowEvent::Char('w'), press(Key::P)], false);
        assert!(keys.is_empty());
        assert!(consumed.iter().all(|c| !c));
    }
    #[test]
    fn paste_works_shut_and_open() {
        let ctrl = WindowEvent::Key(Key::V, 0, Action::Press, Modifiers::Control);
        let (keys, ..) = run(&[ctrl.clone()], false);
        assert_eq!(keys, vec![CK::Paste("pasted".into())]);
        let (keys, ..) = run(&[ctrl], true);
        assert_eq!(keys, vec![CK::Paste("pasted".into())]);
    }
}
