//! How often to redraw, when redrawing is not free.
//!
//! Every animation in the shell currently asks for the next frame as soon as
//! possible, which is right on hardware that composites in a few hundred
//! microseconds. It is wrong where there is no 3D acceleration: a software
//! rasterizer filling 1224x2700 does it in milliseconds of CPU, at whatever
//! rate it is asked, and an animation that could run at 20 Hz without anyone
//! noticing instead runs at 90 and heats the phone.
//!
//! So the shell asks for a frame at an INTERVAL rather than immediately, and
//! the interval depends on two things: whether there is acceleration, and
//! what is moving. Those are different questions and the second one matters
//! even on a GPU, because a slow fade does not need 90 frames a second on
//! any hardware.
//!
//! ## Which animations can be slowed, and which cannot
//!
//! Anything the finger is driving must not be: a drag rendered at 20 Hz
//! feels broken in a way that no amount of smoothness elsewhere excuses,
//! because the hand has a direct comparison — the thing it is touching. So
//! [`Motion::Tracking`] always gets the full rate.
//!
//! Everything else is the shell moving on its own, where the eye has nothing
//! to compare against and a lower rate reads as the chosen speed rather than
//! as a fault.

use crate::Millis;

/// What is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Motion {
    /// Nothing; do not ask for a frame at all.
    Still,
    /// A slow ambient drift: breathing, a gradient shifting with the hour.
    Ambient,
    /// A transition the shell started: a sheet opening, a fade.
    Transition,
    /// The finger is driving it. Never slowed.
    Tracking,
}

/// Frame intervals in milliseconds. The accelerated column is "as fast as
/// the compositor will take it" for tracking, and merely generous elsewhere.
pub fn interval_ms(m: Motion, accelerated: bool) -> Option<Millis> {
    match (m, accelerated) {
        (Motion::Still, _) => None,
        (Motion::Tracking, _) => Some(0),
        (Motion::Transition, true) => Some(0),
        // 40 ms is 25 Hz: fast enough that a 200 ms transition still has
        // five frames, which reads as motion rather than as a jump.
        (Motion::Transition, false) => Some(40),
        (Motion::Ambient, true) => Some(33),
        // A breath takes seconds; 8 Hz is plenty and costs a ninth of 90.
        (Motion::Ambient, false) => Some(125),
    }
}

/// Classify a renderer string. Used once at startup: everything downstream
/// is a bool, because the distinction that matters is "is a full-screen fill
/// cheap or not", not which driver it is.
pub fn accelerated_renderer(gl_renderer: &str) -> bool {
    let r = gl_renderer.to_ascii_lowercase();
    !(r.contains("llvmpipe") || r.contains("softpipe") || r.contains("swrast") || r.contains("soft") && r.contains("pipe") || r.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finger_is_never_made_to_wait() {
        // The hand has a direct comparison with what it is touching, so a
        // slowed drag reads as broken no matter how cheap it is.
        assert_eq!(interval_ms(Motion::Tracking, false), Some(0));
        assert_eq!(interval_ms(Motion::Tracking, true), Some(0));
    }

    #[test]
    fn stillness_asks_for_nothing() {
        assert_eq!(interval_ms(Motion::Still, true), None);
        assert_eq!(interval_ms(Motion::Still, false), None);
    }

    #[test]
    fn software_rendering_slows_only_what_can_be_slowed() {
        let soft = |m| interval_ms(m, false).unwrap();
        let hard = |m| interval_ms(m, true).unwrap();
        assert!(soft(Motion::Ambient) > soft(Motion::Transition), "a breath can be slower than a fade");
        assert!(soft(Motion::Transition) > hard(Motion::Transition));
        assert!(soft(Motion::Ambient) > hard(Motion::Ambient));
        // A 200 ms transition still gets several frames, or it is a jump.
        assert!(200 / soft(Motion::Transition).max(1) >= 5);
    }

    #[test]
    fn the_renderer_string_decides_and_errs_toward_slow() {
        for soft in ["llvmpipe (LLVM 17.0.6, 128 bits)", "softpipe", "SWRAST", ""] {
            assert!(!accelerated_renderer(soft), "{soft:?} should count as software");
        }
        for fast in ["FD643", "Adreno (TM) 643", "Mali-G57", "Mesa Intel(R) UHD Graphics"] {
            assert!(accelerated_renderer(fast), "{fast:?} should count as accelerated");
        }
        // An unknown string is treated as accelerated: the cost of guessing
        // wrong the other way is a shell that feels sluggish on hardware
        // that was fine.
        assert!(accelerated_renderer("Some New GPU 9000"));
    }
}
