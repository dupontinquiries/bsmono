//! fp5-rescued — watches the power and volume-down keys for the rescue chord and, when it sees
//! it, starts `fp5-rescue-console.service`. Root, std only.
//!
//!   fp5-rescued              run (needs read access to /dev/input/event*)
//!   fp5-rescued --probe      list the key devices it would watch, then exit
//!   fp5-rescued --dry-run    run, but print instead of starting the console

mod chord;

use chord::{Chord, Event, Key};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

const EV_KEY: u16 = 1;
const KEY_VOLUMEDOWN: u16 = 114;
const KEY_POWER: u16 = 116;

/// `struct input_event` on 64-bit Linux: two i64 (time), u16 type, u16 code, i32 value.
fn decode(b: &[u8]) -> Option<(u16, u16, i32)> {
    if b.len() < 24 {
        return None;
    }
    Some((u16::from_le_bytes([b[16], b[17]]), u16::from_le_bytes([b[18], b[19]]), i32::from_le_bytes([b[20], b[21], b[22], b[23]])))
}

/// Does the kernel's key bitmap (`capabilities/key`, hex words, highest first) have `code` set?
fn caps_has(caps: &str, code: u16) -> bool {
    let words: Vec<u64> = caps.split_whitespace().rev().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
    words.get(code as usize / 64).is_some_and(|w| w >> (code % 64) & 1 == 1)
}

fn devices() -> Vec<(PathBuf, String)> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir("/sys/class/input") else { return out };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        if !n.starts_with("event") {
            continue;
        }
        let caps = std::fs::read_to_string(e.path().join("device/capabilities/key")).unwrap_or_default();
        if caps_has(&caps, KEY_POWER) || caps_has(&caps, KEY_VOLUMEDOWN) {
            let name = std::fs::read_to_string(e.path().join("device/name")).unwrap_or_default().trim().to_string();
            out.push((PathBuf::from("/dev/input").join(n), name));
        }
    }
    out.sort();
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (probe, dry) = (args.iter().any(|a| a == "--probe"), args.iter().any(|a| a == "--dry-run"));
    let devs = devices();
    if probe {
        for (p, n) in &devs {
            println!("{}  {}", p.display(), n);
        }
        if devs.is_empty() {
            println!("(no power/volume-down device found)");
        }
        return;
    }
    let (tx, rx) = mpsc::channel::<(u16, i32)>();
    let mut opened = 0;
    for (path, name) in devs {
        match std::fs::File::open(&path) {
            Ok(mut f) => {
                opened += 1;
                eprintln!("fp5-rescued: watching {} ({name})", path.display());
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 24 * 16];
                    while let Ok(n) = f.read(&mut buf) {
                        for c in buf[..n].chunks_exact(24) {
                            if let Some((EV_KEY, code, v)) = decode(c) {
                                if v == 0 || v == 1 {
                                    let _ = tx.send((code, v));
                                }
                            }
                        }
                    }
                });
            }
            Err(e) => eprintln!("fp5-rescued: cannot open {}: {e}", path.display()),
        }
    }
    if opened == 0 {
        eprintln!("fp5-rescued: no readable key device; exiting");
        std::process::exit(1);
    }
    drop(tx);
    let t0 = Instant::now();
    let mut chord = Chord::default();
    for (code, v) in rx {
        let key = match code {
            KEY_POWER => Key::Power,
            KEY_VOLUMEDOWN => Key::VolumeDown,
            _ => Key::Other,
        };
        if chord.feed(Event { key, down: v == 1, t_ms: t0.elapsed().as_millis() as u64 }) {
            eprintln!("fp5-rescued: chord - opening the rescue console");
            if !dry {
                let _ = std::process::Command::new("systemctl").args(["--no-block", "start", "fp5-rescue-console.service"]).status();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_words_are_read_highest_first_and_by_the_right_bit() {
        // FP5: gpio-keys has KEY_VOLUMEUP(115) in the second 64-bit word; pmic_resin has VOLUMEDOWN(114).
        let down = format!("{:x} 0", 1u64 << (114 - 64));
        assert!(caps_has(&down, KEY_VOLUMEDOWN) && !caps_has(&down, 115) && !caps_has(&down, KEY_POWER));
        let power = format!("{:x} 0", 1u64 << (116 - 64));
        assert!(caps_has(&power, KEY_POWER) && !caps_has(&power, KEY_VOLUMEDOWN));
        assert!(!caps_has("", KEY_POWER) && !caps_has("zz", KEY_POWER));
    }

    #[test]
    fn an_input_event_decodes_and_a_short_read_does_not() {
        let mut b = [0u8; 24];
        b[16..18].copy_from_slice(&EV_KEY.to_le_bytes());
        b[18..20].copy_from_slice(&KEY_POWER.to_le_bytes());
        b[20..24].copy_from_slice(&1i32.to_le_bytes());
        assert_eq!(decode(&b), Some((EV_KEY, KEY_POWER, 1)));
        assert_eq!(decode(&b[..20]), None);
    }
}
