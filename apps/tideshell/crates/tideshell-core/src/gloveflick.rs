//! Glove flicks: steering the shell with the phone itself, for cold mode.
//!
//! Gloves do not work on glass, and a held finger is exactly what cold mode
//! cannot count on, so the older tilt-select (tilt.rs: gyro, finger on the
//! palette) is no help there. This is the other half: the flick ratchet from
//! uikit-egui's glove.rs (flick out, come back to rest = one step; works from
//! the accelerometer or iio-sensor-proxy's orientation strings, no gyro),
//! turned into the same intents the volume keys already have.
//!
//! One rule decides who a flick is for:
//! * palette OPEN: up/down walk the selection, right activates, left closes;
//! * palette CLOSED: up/down open it and take the first step (as the volume
//!   keys do), and left/right go to the keyboard's voice typing as
//!   Done / Cancel (a keyboard that is not dictating ignores them);
//! * holding the phone face-down (the glove module's escape) closes the palette
//!   and cancels dictation: the "stop everything" gesture.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flick {
    Up,
    Down,
    Left,
    Right,
    /// Held face-down past the glove module's escape time.
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GloveIntent {
    PaletteOpenAndStep { down: bool },
    PaletteStep { down: bool },
    PaletteActivate,
    PaletteClose,
    /// Commands for rusty-board's control socket.
    Keyboard(&'static str),
    EscapeAll,
}

pub fn intent(f: Flick, palette_open: bool) -> GloveIntent {
    use GloveIntent as G;
    match (f, palette_open) {
        (Flick::Escape, _) => G::EscapeAll,
        (Flick::Up, true) => G::PaletteStep { down: false },
        (Flick::Down, true) => G::PaletteStep { down: true },
        (Flick::Right, true) => G::PaletteActivate,
        (Flick::Left, true) => G::PaletteClose,
        (Flick::Up, false) => G::PaletteOpenAndStep { down: false },
        (Flick::Down, false) => G::PaletteOpenAndStep { down: true },
        (Flick::Right, false) => G::Keyboard("voice-done"),
        (Flick::Left, false) => G::Keyboard("voice-cancel"),
    }
}

/// A screen-space step direction (x right, y down) as a flick. The glove
/// module reports which way the phone was turned as a unit vector.
pub fn flick_from_direction(dx: f32, dy: f32) -> Option<Flick> {
    if dx.abs() < 0.5 && dy.abs() < 0.5 {
        return None;
    }
    Some(if dx.abs() > dy.abs() {
        if dx > 0.0 {
            Flick::Right
        } else {
            Flick::Left
        }
    } else if dy > 0.0 {
        Flick::Down
    } else {
        Flick::Up
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_owns_flicks_when_open_and_the_keyboard_gets_left_right_when_not() {
        assert_eq!(intent(Flick::Down, true), GloveIntent::PaletteStep { down: true });
        assert_eq!(intent(Flick::Right, true), GloveIntent::PaletteActivate);
        assert_eq!(intent(Flick::Left, true), GloveIntent::PaletteClose);
        assert_eq!(intent(Flick::Up, false), GloveIntent::PaletteOpenAndStep { down: false });
        assert_eq!(intent(Flick::Right, false), GloveIntent::Keyboard("voice-done"));
        assert_eq!(intent(Flick::Left, false), GloveIntent::Keyboard("voice-cancel"));
        assert_eq!(intent(Flick::Escape, true), GloveIntent::EscapeAll);
    }

    #[test]
    fn directions_map_to_the_dominant_axis() {
        assert_eq!(flick_from_direction(1.0, 0.0), Some(Flick::Right));
        assert_eq!(flick_from_direction(0.0, -1.0), Some(Flick::Up));
        assert_eq!(flick_from_direction(-0.8, 0.3), Some(Flick::Left));
        assert_eq!(flick_from_direction(0.0, 0.0), None);
    }
}
