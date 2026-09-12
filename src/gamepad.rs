// Controller support (optional, `--features gamepad`, uses gilrs — needs
// libudev-dev on Linux). Sticks have fixed roles (left = move, right = look,
// RT = attack); every button goes through the rebindable input system — open
// the controls panel and press a controller button to bind it. Without the
// feature this is a stub that reports no pad. Same shape as the monorepo's
// shared `gamepad.rs` (skydribble / blackstar-rs) so the save files agree.

/// Stable button codes shared by bindings + the config file.
pub const PB_SOUTH: u32 = 0;
pub const PB_EAST: u32 = 1;
pub const PB_WEST: u32 = 2;
pub const PB_NORTH: u32 = 3;
pub const PB_LB: u32 = 4;
pub const PB_RB: u32 = 5;
pub const PB_LT: u32 = 6; // trigger past 55% also reads as a button
pub const PB_RT: u32 = 7;
pub const PB_SELECT: u32 = 8;
pub const PB_START: u32 = 9;
pub const PB_L3: u32 = 10;
pub const PB_R3: u32 = 11;
pub const PB_DUP: u32 = 12;
pub const PB_DDOWN: u32 = 13;
pub const PB_DLEFT: u32 = 14;
pub const PB_DRIGHT: u32 = 15;

pub fn pad_button_name(code: u32) -> &'static str {
    match code {
        PB_SOUTH => "A",
        PB_EAST => "B",
        PB_WEST => "X",
        PB_NORTH => "Y",
        PB_LB => "LB",
        PB_RB => "RB",
        PB_LT => "LT",
        PB_RT => "RT",
        PB_SELECT => "SELECT",
        PB_START => "START",
        PB_L3 => "L3",
        PB_R3 => "R3",
        PB_DUP => "D-UP",
        PB_DDOWN => "D-DOWN",
        PB_DLEFT => "D-LEFT",
        PB_DRIGHT => "D-RIGHT",
        _ => "?",
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct PadState {
    pub connected: bool,
    pub stick_x: f32, // left stick, dead-zoned, y up
    pub stick_y: f32,
    pub rstick_x: f32, // right stick, dead-zoned
    pub rstick_y: f32,
    pub throttle: f32, // RT - LT
    pub pressed: u32,  // bitmask by PB_* code
    pub just_pressed: u32,
    pub start_pressed: bool,
}

impl PadState {
    pub fn down(&self, code: u32) -> bool {
        self.pressed & (1 << code) != 0
    }
    pub fn edge(&self, code: u32) -> bool {
        self.just_pressed & (1 << code) != 0
    }
    /// Lowest just-pressed button, for bind capture.
    pub fn any_edge(&self) -> Option<u32> {
        (0..16).find(|c| self.edge(*c))
    }
}

/// Radial dead zone with re-scaling so the first millimetre past the zone
/// isn't a jump.
pub fn dead_zone(x: f32, y: f32, dead: f32) -> (f32, f32) {
    let len = (x * x + y * y).sqrt();
    if len < dead {
        return (0.0, 0.0);
    }
    let mag = ((len - dead) / (1.0 - dead)).clamp(0.0, 1.0);
    (x / len * mag, y / len * mag)
}

#[cfg(feature = "gamepad")]
mod imp {
    use super::*;
    use gilrs::{Axis, Button, Gilrs};

    fn code_of(b: Button) -> Option<u32> {
        Some(match b {
            Button::South => PB_SOUTH,
            Button::East => PB_EAST,
            Button::West => PB_WEST,
            Button::North => PB_NORTH,
            Button::LeftTrigger => PB_LB,
            Button::RightTrigger => PB_RB,
            Button::LeftTrigger2 => PB_LT,
            Button::RightTrigger2 => PB_RT,
            Button::Select => PB_SELECT,
            Button::Start => PB_START,
            Button::LeftThumb => PB_L3,
            Button::RightThumb => PB_R3,
            Button::DPadUp => PB_DUP,
            Button::DPadDown => PB_DDOWN,
            Button::DPadLeft => PB_DLEFT,
            Button::DPadRight => PB_DRIGHT,
            _ => return None,
        })
    }

    pub struct Pad {
        g: Option<Gilrs>,
        prev: u32,
    }

    impl Pad {
        pub fn new() -> Pad {
            let g = Gilrs::new().ok();
            if g.is_none() {
                eprintln!("[clawbound] gamepad: gilrs init failed — keyboard/mouse only");
            }
            Pad { g, prev: 0 }
        }

        pub fn poll(&mut self) -> PadState {
            let Some(g) = self.g.as_mut() else { return PadState::default() };
            while g.next_event().is_some() {}
            let Some((_, gp)) = g.gamepads().next() else {
                self.prev = 0;
                return PadState::default();
            };
            let ax = |a: Axis| gp.axis_data(a).map(|d| d.value()).unwrap_or(0.0);
            let tv = |b: Button| gp.button_data(b).map(|d| d.value()).unwrap_or(0.0);
            let mut pressed = 0u32;
            for b in [
                Button::South,
                Button::East,
                Button::West,
                Button::North,
                Button::LeftTrigger,
                Button::RightTrigger,
                Button::Select,
                Button::Start,
                Button::LeftThumb,
                Button::RightThumb,
                Button::DPadUp,
                Button::DPadDown,
                Button::DPadLeft,
                Button::DPadRight,
            ] {
                if gp.is_pressed(b) {
                    if let Some(c) = code_of(b) {
                        pressed |= 1 << c;
                    }
                }
            }
            if tv(Button::LeftTrigger2) > 0.55 {
                pressed |= 1 << PB_LT;
            }
            if tv(Button::RightTrigger2) > 0.55 {
                pressed |= 1 << PB_RT;
            }
            let just = pressed & !self.prev;
            self.prev = pressed;
            let (sx, sy) = dead_zone(ax(Axis::LeftStickX), ax(Axis::LeftStickY), 0.14);
            let (rx, ry) = dead_zone(ax(Axis::RightStickX), ax(Axis::RightStickY), 0.12);
            PadState {
                connected: true,
                stick_x: sx,
                stick_y: sy,
                rstick_x: rx,
                rstick_y: ry,
                throttle: (tv(Button::RightTrigger2) - tv(Button::LeftTrigger2)).clamp(-1.0, 1.0),
                pressed,
                just_pressed: just,
                start_pressed: just & (1 << PB_START) != 0,
            }
        }
    }
}

#[cfg(not(feature = "gamepad"))]
mod imp {
    use super::PadState;

    pub struct Pad;

    impl Pad {
        pub fn new() -> Pad {
            Pad
        }
        pub fn poll(&mut self) -> PadState {
            PadState::default()
        }
    }
}

pub use imp::Pad;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_zone_is_radial_and_rescaled() {
        assert_eq!(dead_zone(0.05, 0.05, 0.14), (0.0, 0.0));
        let (x, y) = dead_zone(1.0, 0.0, 0.14);
        assert!((x - 1.0).abs() < 1e-5 && y == 0.0);
        let (x, _) = dead_zone(0.15, 0.0, 0.14);
        assert!(x > 0.0 && x < 0.05, "just past the zone is tiny, not a jump: {x}");
    }
}
