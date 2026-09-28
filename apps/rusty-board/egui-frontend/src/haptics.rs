//! haptics — plays `haptic_kit::Pattern`s on the vibration motor, off the
//! UI thread. A Morse word is seconds long; the keyboard must keep drawing
//! (and taking the next word) meanwhile, so a worker plays a queue.
//!
//! A new pattern REPLACES whatever is playing: feedback for the word you
//! just swiped matters, the tail of the previous one does not.
//!
//! No motor (a desktop, or a phone whose vibrator is not a sysfs `leds`
//! device) = every call is a no-op. That second case matters: some phones
//! drive the motor through an input force-feedback device instead, which
//! `haptic_kit` does not speak yet.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

pub struct Haptics {
    tx: Option<Sender<haptic_kit::Pattern>>,
}

impl Haptics {
    /// Find the motor and start the worker; a no-op handle without one.
    pub fn start() -> Haptics {
        let Ok(motor) = haptic_kit::Motor::discover() else { return Haptics { tx: None } };
        let (tx, rx) = channel();
        let ok = std::thread::Builder::new().name("rb-haptics".into()).spawn(move || worker(motor, rx)).is_ok();
        Haptics { tx: ok.then_some(tx) }
    }

    /// A handle that does nothing (tests, and no motor).
    pub fn none() -> Haptics {
        Haptics { tx: None }
    }

    pub fn available(&self) -> bool {
        self.tx.is_some()
    }

    pub fn play(&self, p: haptic_kit::Pattern) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(p);
        }
    }
}

fn worker(motor: haptic_kit::Motor, rx: Receiver<haptic_kit::Pattern>) {
    let mut next: Option<haptic_kit::Pattern> = None;
    loop {
        let p = match next.take() {
            Some(p) => p,
            None => match rx.recv() {
                Ok(p) => p,
                Err(_) => return,
            },
        };
        'play: for pulse in &p.pulses {
            for (on, ms) in [(true, pulse.on_ms), (false, pulse.off_ms)] {
                if ms == 0 {
                    continue;
                }
                let _ = if on { motor.set(1.0) } else { motor.off() };
                match rx.recv_timeout(Duration::from_millis(ms as u64)) {
                    Ok(newer) => {
                        next = Some(newer);
                        break 'play;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        let _ = motor.off();
                        return;
                    }
                }
            }
        }
        let _ = motor.off();
    }
}
