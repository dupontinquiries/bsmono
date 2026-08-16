//! midi — hardware controllers, arriving as the same notes the keyboard makes.
//!
//! Two layers, split at the line that matters: [`parse`] turns raw MIDI
//! bytes into messages and is pure, so every wire-format decision (running
//! status, note-on-with-zero-velocity, 14-bit bend) is tested without a
//! controller plugged in; [`MidiIn`] is the `midir` connection that feeds it
//! and is feature-gated, so a build without ALSA still compiles and simply
//! reports no ports.
//!
//! ## What a note becomes
//!
//! A `NoteOn` becomes [`studio_egui::playkeys::PlayNote`] — the same value
//! the computer keyboard produces — so the shell has ONE note path: voice it
//! through the sink, and land it in the pattern when recording. A second
//! path for MIDI would be a second place for velocity, tuning, and channel
//! selection to drift apart.
//!
//! ## Notes, not clocks
//!
//! Note on/off, CC, and pitch bend are parsed. MIDI CLOCK is deliberately
//! not: syncing this transport to an external clock means resampling the
//! render against drifting wall time, which is a real engineering problem
//! (and belongs with the streaming engine), not a `0xF8` match arm.

use std::sync::mpsc;

/// One decoded MIDI message. Channel is 0-based (the wire's 0..15), unlike
/// the 1..16 hardware displays — converted at the UI edge, never here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiMessage {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    /// -1.0..=1.0, from the wire's 14-bit 0..16383 with 8192 as centre.
    PitchBend { channel: u8, bend: f32 },
}

/// Decode one MIDI packet. `midir` delivers whole messages, but running
/// status (a stream that omits the repeated status byte) still appears from
/// some hardware and from files, so `running` carries the last status seen.
pub fn parse(bytes: &[u8], running: &mut Option<u8>) -> Option<MidiMessage> {
    if bytes.is_empty() { return None; }
    let (status, data) = if bytes[0] & 0x80 != 0 {
        // Real-time messages (0xF8..=0xFF) may interleave anywhere and must
        // NOT clobber running status — that is the classic parser bug that
        // turns a clock tick into a stuck note.
        if bytes[0] >= 0xF8 { return None; }
        *running = Some(bytes[0]);
        (bytes[0], &bytes[1..])
    } else {
        (( *running)?, bytes)
    };
    let channel = status & 0x0F;
    match status & 0xF0 {
        0x90 => {
            let note = *data.first()?;
            let velocity = *data.get(1)?;
            // Note-on with velocity 0 IS note-off — the convention most
            // controllers actually send, and treating it as a note would
            // leave every key stuck on.
            if velocity == 0 {
                Some(MidiMessage::NoteOff { channel, note })
            } else {
                Some(MidiMessage::NoteOn { channel, note, velocity })
            }
        }
        0x80 => Some(MidiMessage::NoteOff { channel, note: *data.first()? }),
        0xB0 => Some(MidiMessage::ControlChange {
            channel, controller: *data.first()?, value: *data.get(1)?,
        }),
        0xE0 => {
            let lsb = *data.first()? as u16;
            let msb = *data.get(1)? as u16;
            let raw = (msb << 7) | lsb;
            Some(MidiMessage::PitchBend {
                channel,
                bend: (raw as f32 - 8192.0) / 8192.0,
            })
        }
        _ => None,
    }
}

/// MIDI note number → the rack's semitone offset. Middle C (60) is the
/// rack's 0, matching the channel's own tuning origin, so a controller and
/// the computer keyboard agree about what "no transposition" means.
pub fn semitone_of(note: u8) -> i8 {
    (note as i16 - 60).clamp(-128, 127) as i8
}

/// A note as the rest of the shell already understands it.
pub fn play_note(note: u8, velocity: u8) -> studio_egui::playkeys::PlayNote {
    studio_egui::playkeys::PlayNote {
        semitone: semitone_of(note),
        velocity: (velocity.max(1) as f32 / 127.0).clamp(0.0, 1.0),
    }
}

/// An open MIDI input. Messages arrive on a channel the shell drains per
/// frame — the same shape the TCP carrier uses, and for the same reason: the
/// `Studio` stays single-threaded and nothing in the UI blocks on hardware.
pub struct MidiIn {
    rx: mpsc::Receiver<MidiMessage>,
    port_name: String,
    #[cfg(feature = "midi")]
    _conn: midir::MidiInputConnection<()>,
}

impl MidiIn {
    /// Open the first available input port, or the first whose name contains
    /// `prefer` (case-insensitive) — the practical version of a device
    /// picker: most people have one controller, and the ones who have three
    /// know its name.
    #[cfg(feature = "midi")]
    pub fn open(prefer: Option<&str>) -> Result<MidiIn, String> {
        use midir::{Ignore, MidiInput};
        let mut input = MidiInput::new("studio").map_err(|e| e.to_string())?;
        // Sysex, timing and active-sensing traffic would otherwise flood the
        // channel with messages `parse` discards anyway.
        input.ignore(Ignore::All);
        let ports = input.ports();
        if ports.is_empty() { return Err("no MIDI input ports".into()); }
        let chosen = prefer
            .and_then(|want| {
                let want = want.to_lowercase();
                ports.iter().find(|p| input.port_name(p)
                    .map(|n| n.to_lowercase().contains(&want))
                    .unwrap_or(false))
            })
            .unwrap_or(&ports[0])
            .clone();
        let port_name = input.port_name(&chosen).unwrap_or_else(|_| "midi".into());
        let (tx, rx) = mpsc::channel();
        let conn = input
            .connect(&chosen, "studio-in", move |_stamp, bytes, _| {
                // Running status is per-connection state, owned by this
                // callback alone.
                let mut running = None;
                if let Some(m) = parse(bytes, &mut running) {
                    let _ = tx.send(m);
                }
            }, ())
            .map_err(|e| e.to_string())?;
        Ok(MidiIn { rx, port_name, _conn: conn })
    }

    #[cfg(not(feature = "midi"))]
    pub fn open(_prefer: Option<&str>) -> Result<MidiIn, String> {
        Err("built without the `midi` feature".into())
    }

    /// Everything that arrived since the last poll.
    pub fn poll(&self) -> Vec<MidiMessage> {
        self.rx.try_iter().collect()
    }

    pub fn port_name(&self) -> &str { &self.port_name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_on_with_zero_velocity_is_a_note_off() {
        // The convention most controllers send. Miss it and every key sticks.
        let mut r = None;
        assert_eq!(parse(&[0x90, 60, 100], &mut r),
                   Some(MidiMessage::NoteOn { channel: 0, note: 60, velocity: 100 }));
        assert_eq!(parse(&[0x90, 60, 0], &mut r),
                   Some(MidiMessage::NoteOff { channel: 0, note: 60 }));
        assert_eq!(parse(&[0x83, 64, 0], &mut r),
                   Some(MidiMessage::NoteOff { channel: 3, note: 64 }));
    }

    #[test]
    fn running_status_continues_and_a_clock_tick_does_not_break_it() {
        // The classic parser bug: a 0xF8 clock byte overwriting running
        // status, so the next data-only packet decodes as garbage — or as a
        // note that never ends.
        let mut r = None;
        assert!(parse(&[0x90, 60, 100], &mut r).is_some());
        assert_eq!(parse(&[62, 90], &mut r),
                   Some(MidiMessage::NoteOn { channel: 0, note: 62, velocity: 90 }),
                   "a data-only packet continues the last status");
        assert_eq!(parse(&[0xF8], &mut r), None, "a clock tick is not a message");
        assert_eq!(parse(&[64, 80], &mut r),
                   Some(MidiMessage::NoteOn { channel: 0, note: 64, velocity: 80 }),
                   "and it did not clobber running status");
        // Data with no status ever seen is not guessed at.
        let mut fresh = None;
        assert_eq!(parse(&[60, 100], &mut fresh), None);
    }

    #[test]
    fn pitch_bend_is_fourteen_bits_centred_at_zero() {
        let mut r = None;
        match parse(&[0xE0, 0x00, 0x40], &mut r) {
            Some(MidiMessage::PitchBend { bend, .. }) =>
                assert!(bend.abs() < 1e-6, "8192 is centre, got {bend}"),
            other => panic!("expected bend, got {other:?}"),
        }
        match parse(&[0xE0, 0x7F, 0x7F], &mut r) {
            Some(MidiMessage::PitchBend { bend, .. }) =>
                assert!((bend - 1.0).abs() < 0.001, "max is +1, got {bend}"),
            other => panic!("expected bend, got {other:?}"),
        }
        match parse(&[0xE0, 0x00, 0x00], &mut r) {
            Some(MidiMessage::PitchBend { bend, .. }) =>
                assert!((bend + 1.0).abs() < 1e-6, "min is -1, got {bend}"),
            other => panic!("expected bend, got {other:?}"),
        }
    }

    #[test]
    fn middle_c_is_the_racks_origin_and_velocity_never_silences_a_note() {
        assert_eq!(semitone_of(60), 0, "a controller's middle C is the rack's 0");
        assert_eq!(semitone_of(72), 12);
        assert_eq!(semitone_of(48), -12);
        let n = play_note(60, 127);
        assert_eq!(n.semitone, 0);
        assert!((n.velocity - 1.0).abs() < 1e-6);
        // Velocity 1 is the quietest playable note, not silence — a note
        // that arrives inaudible looks like a broken controller.
        assert!(play_note(60, 1).velocity > 0.0);
    }

    #[test]
    fn a_truncated_packet_is_dropped_rather_than_half_read() {
        let mut r = None;
        assert_eq!(parse(&[0x90, 60], &mut r), None, "no velocity byte, no note");
        assert_eq!(parse(&[0xB0], &mut r), None);
        assert_eq!(parse(&[], &mut r), None);
    }
}
