//! Replays the event ORDER of unendlich_fullgame's two drains, to check
//! that no key reaches movement while the chat has focus — including
//! within the same frame as the Enter that opened it.
#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ev { Enter, Esc, Key(char), Char(char) }

#[derive(Default)]
pub struct Sim {
    pub chat_focus: bool,
    /// Keys that reached the movement system. Must stay empty while typing.
    pub to_movement: Vec<char>,
    pub to_chat: Vec<char>,
    pub released_all: usize,
}

impl Sim {
    /// Main drain (line ~3710) then late drain (line ~12823), same frame.
    pub fn frame(&mut self, events: &[Ev]) {
        for e in events {
            // main drain: input.handle_event guarded by chat_focus
            match *e {
                Ev::Enter => {
                    if self.chat_focus { self.chat_focus = false; }
                    else { self.chat_focus = true; self.released_all += 1; }
                }
                Ev::Esc => { if self.chat_focus { self.chat_focus = false; } }
                Ev::Key(c) => { if !self.chat_focus { self.to_movement.push(c); } }
                Ev::Char(c) => {
                    if self.chat_focus { self.to_chat.push(c); }
                }
            }
        }
        // late drain: same guard, or the keys leak here instead
        for e in events {
            if let Ev::Key(c) = *e {
                if !self.chat_focus { self.to_movement.push(c); }
            }
        }
    }
}
