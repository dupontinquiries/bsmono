//! analog-kit — analog keyboard depth, as a value.
//!
//! Wooting analog keyboards report how FAR a key is pressed, 0..1, instead of
//! only whether it is down. This crate is the reusable half of that: load the
//! installed Wooting Analog SDK runtime by its documented name, read the
//! buffer, and hand back HID-usage → depth. Nothing here knows about a UI
//! framework — the caller maps its own key events to HID usage codes and
//! keeps its own bindings, because winit's `KeyCode` and egui's `Key` are
//! different enums and neither belongs in a shared audio/input crate.
//!
//! `games/skydribble/src/analog.rs` implemented this first, against winit,
//! and its design is the one adopted here: dlopen so a machine without the
//! runtime simply falls back to digital, learn each key's HID code the first
//! time it is pressed (bindings are logical characters, the SDK speaks HID,
//! and only the person's actual layout knows the mapping), and keep the
//! response curve a pure function. That game is not refactored onto this
//! crate in the same breath — it works, and a drive-by rewrite of working
//! input code is how input regressions arrive.
//!
//! Without the `sdk` feature (or without the runtime installed) every method
//! still answers: `active()` is false and `depth()` returns the digital
//! value. Callers need no cfg of their own.

use std::collections::HashMap;

/// USB HID usage page 7 codes, for the keys anyone would plausibly play.
pub mod hid {
    pub const A: u16 = 0x04;
    pub const Z: u16 = 0x1D;
    pub const DIGIT1: u16 = 0x1E;
    pub const DIGIT0: u16 = 0x27;
    pub const SPACE: u16 = 0x2C;
    pub const SEMICOLON: u16 = 0x33;
    pub const QUOTE: u16 = 0x34;
    pub const COMMA: u16 = 0x36;
    pub const PERIOD: u16 = 0x37;
    pub const SLASH: u16 = 0x38;
    pub const BRACKET_LEFT: u16 = 0x2F;
    pub const BRACKET_RIGHT: u16 = 0x30;
    pub const MINUS: u16 = 0x2D;
    pub const EQUAL: u16 = 0x2E;
}

/// The analog layer. Cheap to construct; construct one and keep it.
pub struct Analog {
    imp: imp::Sdk,
    values: HashMap<u16, f32>,
}

impl Default for Analog {
    fn default() -> Self { Analog::new() }
}

impl Analog {
    pub fn new() -> Analog {
        Analog { imp: imp::Sdk::new(), values: HashMap::new() }
    }

    /// Is a real analog keyboard answering?
    pub fn active(&self) -> bool { self.imp.active() }

    /// A short line for a status bar — an input path you cannot see is an
    /// input path you cannot debug.
    pub fn describe(&self) -> &'static str {
        if self.active() { "analog keys" } else { "digital keys" }
    }

    /// Refresh every currently-depressed analog key. Call once per frame.
    pub fn poll(&mut self) {
        self.values.clear();
        self.imp.read_into(&mut self.values);
    }

    /// Depth for one HID code, 0..1. `held` is the digital state, used when
    /// the key is not analog, not learned, or no runtime is present — so a
    /// caller writes one code path for both kinds of keyboard.
    pub fn depth(&self, hid: Option<u16>, held: bool) -> f32 {
        if self.active() {
            if let Some(h) = hid {
                if let Some(v) = self.values.get(&h) {
                    return *v;
                }
            }
        }
        if held { 1.0 } else { 0.0 }
    }
}

/// Depth → musical velocity, 0..1.
///
/// Deliberately NOT skydribble's `wind` curve: that one keeps a high floor
/// so a light press still turns the car hard, which is right for steering
/// and wrong for playing — a keyboard that cannot play quietly is a keyboard
/// with one velocity. This is the complementary shape: a small deadzone so
/// resting fingers are silent, then a gamma curve that spends most of its
/// travel in the soft half (where expression lives), reaching 1.0 at a full
/// press. `gamma` 1.0 is linear; the 1.6 default suits piano-style playing.
pub fn velocity(depth: f32, gamma: f32) -> f32 {
    const DZ: f32 = 0.06;
    if depth <= DZ { return 0.0; }
    if depth >= 0.97 { return 1.0; }
    let t = (depth - DZ) / (0.97 - DZ);
    t.powf(gamma.max(0.1)).clamp(0.0, 1.0)
}

/// MIDI velocity 1..=127 from a 0..1 value. Zero maps to 1, not 0: a
/// note-on with velocity 0 IS a note-off in MIDI, and a silent note-on is
/// not what a played key means.
pub fn midi_velocity(v: f32) -> u8 {
    ((v.clamp(0.0, 1.0) * 126.0) as u8) + 1
}

// ---------------------------------------------------------------- loader ----
#[cfg(feature = "sdk")]
mod imp {
    use std::collections::HashMap;
    use std::os::raw::{c_int, c_uint};

    type FnInit = unsafe extern "C" fn() -> c_int;
    type FnMode = unsafe extern "C" fn(c_uint) -> c_int;
    type FnBuf = unsafe extern "C" fn(*mut u16, *mut f32, c_uint) -> c_int;

    pub struct Sdk {
        lib: Option<libloading::Library>,
        active: bool,
    }

    impl Sdk {
        pub fn new() -> Sdk {
            let names = [
                "libwooting_analog_sdk.so",
                "wooting_analog_sdk.dll",
                "libwooting_analog_sdk.dylib",
            ];
            for name in names {
                // SAFETY: loading the official SDK runtime by its documented
                // name and calling its documented C entry points.
                let lib = match unsafe { libloading::Library::new(name) } {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                unsafe {
                    let Ok(init) = lib.get::<FnInit>(b"wooting_analog_initialise\0") else { continue };
                    let Ok(mode) = lib.get::<FnMode>(b"wooting_analog_set_keycode_mode\0") else { continue };
                    if lib.get::<FnBuf>(b"wooting_analog_read_full_buffer\0").is_err() { continue }
                    let devices = init();
                    if devices < 0 {
                        return Sdk { lib: Some(lib), active: false };
                    }
                    let _ = mode(0); // 0 = HID usage codes
                    return Sdk { lib: Some(lib), active: true };
                }
            }
            Sdk { lib: None, active: false }
        }
        pub fn active(&self) -> bool { self.active }
        pub fn read_into(&self, out: &mut HashMap<u16, f32>) {
            if !self.active { return }
            let Some(lib) = &self.lib else { return };
            // 32 simultaneous analog keys is more fingers than exist; the
            // SDK truncates rather than failing if somehow exceeded.
            const N: usize = 32;
            let mut codes = [0u16; N];
            let mut vals = [0f32; N];
            // SAFETY: documented symbol, correctly-sized output buffers.
            let n = unsafe {
                let Ok(f) = lib.get::<FnBuf>(b"wooting_analog_read_full_buffer\0") else { return };
                f(codes.as_mut_ptr(), vals.as_mut_ptr(), N as u32)
            };
            for i in 0..(n.max(0) as usize).min(N) {
                out.insert(codes[i], vals[i]);
            }
        }
    }
}

#[cfg(not(feature = "sdk"))]
mod imp {
    use std::collections::HashMap;
    pub struct Sdk;
    impl Sdk {
        pub fn new() -> Sdk { Sdk }
        pub fn active(&self) -> bool { false }
        pub fn read_into(&self, _out: &mut HashMap<u16, f32>) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_keyboard_every_key_is_its_digital_self() {
        // The property that lets callers write one code path: no runtime,
        // no cfg, no branch — just held or not.
        let a = Analog::new();
        if !a.active() {
            assert_eq!(a.depth(Some(hid::A), true), 1.0);
            assert_eq!(a.depth(Some(hid::A), false), 0.0);
            assert_eq!(a.depth(None, true), 1.0);
        }
    }

    #[test]
    fn velocity_is_silent_at_rest_full_at_the_bottom_and_soft_in_between() {
        assert_eq!(velocity(0.0, 1.6), 0.0);
        assert_eq!(velocity(0.03, 1.6), 0.0, "resting fingers do not play notes");
        assert_eq!(velocity(1.0, 1.6), 1.0);
        let half = velocity(0.5, 1.6);
        assert!(half > 0.1 && half < 0.5,
                "a half press is a soft note, not a medium one: {half}");
        // Monotone: harder is always louder, or the keyboard is unplayable.
        let mut prev = -1.0;
        for i in 0..=100 {
            let v = velocity(i as f32 / 100.0, 1.6);
            assert!(v >= prev, "velocity dipped at {i}");
            prev = v;
        }
    }

    #[test]
    fn midi_velocity_never_emits_a_note_off_by_accident() {
        // Velocity 0 on a note-on means note-OFF in MIDI; a played key must
        // never mean that.
        assert_eq!(midi_velocity(0.0), 1);
        assert_eq!(midi_velocity(1.0), 127);
        assert!(midi_velocity(0.5) > 1 && midi_velocity(0.5) < 127);
    }
}
