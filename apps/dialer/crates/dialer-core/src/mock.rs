//! The simulated modem. This is the desktop/demo backend, not a test-only
//! stub: a dev laptop has no modem, and the whole UI must be exercisable
//! there. It speaks the identical [`Link`] protocol as the mm backend.
//!
//! The loop is a plain `recv_timeout` tick you can read top to bottom:
//! every 250 ms it advances scripted call phases; commands interleave
//! whenever they arrive. Scripts:
//!
//! * outgoing: Dialing (1 s) → RingingOut (2 s) → Active; hang up ends it.
//!   Numbers ending in `0` are never answered (so NoAnswer is reachable).
//! * `SimulateIncoming(n)`: RingingIn until accepted/rejected; if ignored
//!   for 15 s the "caller" gives up (Missed becomes reachable).
//! * active calls echo DTMF as an `Error`-free no-op (logged via Event only
//!   in doc terms — the modem would play the tone; there is nothing to hear).

use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::link::{Command, Event, Link};
use crate::types::{CallDirection, CallInfo, CallState};

const TICK: Duration = Duration::from_millis(250);

struct MockCall {
    info: CallInfo,
    born: Instant,
    /// For incoming calls: when the caller gives up.
    patience: Option<Duration>,
}

/// Spawn the simulator thread and hand back the link.
pub fn spawn() -> Link {
    let (cmd_tx, cmd_rx) = channel::<Command>();
    let (ev_tx, ev_rx) = channel::<Event>();

    std::thread::Builder::new()
        .name("dialer-mock".into())
        .spawn(move || {
            let _ = ev_tx.send(Event::ModemUp { label: "simulated modem".into() });
            let mut calls: Vec<MockCall> = Vec::new();
            let mut next_id = 0u32;

            loop {
                // 1) one command, if any arrived within a tick
                match cmd_rx.recv_timeout(TICK) {
                    Ok(Command::Quit) => return,
                    Ok(Command::Dial(number)) => {
                        next_id += 1;
                        let info = CallInfo {
                            id: format!("mock/{next_id}"),
                            number: crate::number::normalize(&number),
                            direction: CallDirection::Outgoing,
                            state: CallState::Dialing,
                        };
                        calls.push(MockCall { info: info.clone(), born: Instant::now(), patience: None });
                        let _ = ev_tx.send(Event::CallAdded(info));
                    }
                    Ok(Command::SimulateIncoming(number)) => {
                        next_id += 1;
                        let info = CallInfo {
                            id: format!("mock/{next_id}"),
                            number: crate::number::normalize(&number),
                            direction: CallDirection::Incoming,
                            state: CallState::RingingIn,
                        };
                        calls.push(MockCall {
                            info: info.clone(),
                            born: Instant::now(),
                            patience: Some(Duration::from_secs(15)),
                        });
                        let _ = ev_tx.send(Event::CallAdded(info));
                    }
                    Ok(Command::Accept(id)) => {
                        if let Some(c) = calls.iter_mut().find(|c| c.info.id == id) {
                            if c.info.state.is_ringing_in() {
                                c.info.state = CallState::Active;
                                let _ = ev_tx.send(Event::CallState { id, state: CallState::Active });
                            }
                        }
                    }
                    Ok(Command::Hangup(id)) => {
                        if let Some(pos) = calls.iter().position(|c| c.info.id == id) {
                            calls.remove(pos);
                            let _ = ev_tx.send(Event::CallState { id, state: CallState::Terminated });
                        }
                    }
                    Ok(Command::Dtmf(_, _)) => { /* a real modem plays the tone; nothing to do */ }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return, // shell dropped the link
                }

                // 2) advance scripts
                let mut ended: Vec<String> = Vec::new();
                for c in calls.iter_mut() {
                    let age = c.born.elapsed();
                    match c.info.state {
                        CallState::Dialing if age >= Duration::from_secs(1) => {
                            c.info.state = CallState::RingingOut;
                            let _ = ev_tx
                                .send(Event::CallState { id: c.info.id.clone(), state: CallState::RingingOut });
                        }
                        CallState::RingingOut if age >= Duration::from_secs(3) => {
                            if c.info.number.ends_with('0') {
                                // scripted no-answer after 10 s of ringing
                                if age >= Duration::from_secs(10) {
                                    ended.push(c.info.id.clone());
                                }
                            } else {
                                c.info.state = CallState::Active;
                                let _ = ev_tx
                                    .send(Event::CallState { id: c.info.id.clone(), state: CallState::Active });
                            }
                        }
                        CallState::RingingIn => {
                            if let Some(p) = c.patience {
                                if age >= p {
                                    ended.push(c.info.id.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                for id in ended {
                    calls.retain(|c| c.info.id != id);
                    let _ = ev_tx.send(Event::CallState { id, state: CallState::Terminated });
                }
            }
        })
        .expect("spawn dialer-mock thread");

    Link { tx: cmd_tx, rx: ev_rx, kind: "mock" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full lifecycle against the real thread, with generous timeouts so a
    /// loaded CI machine does not flake: dial → active → hangup → terminated.
    #[test]
    fn dial_progresses_to_active_and_hangup_terminates() {
        let link = spawn();
        // ModemUp arrives first.
        let ev = link.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(ev, Event::ModemUp { .. }));

        link.tx.send(Command::Dial("555-1231".into())).unwrap();
        let mut saw_active = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut call_id = String::new();
        while Instant::now() < deadline {
            match link.rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Event::CallAdded(info)) => {
                    assert_eq!(info.number, "5551231");
                    call_id = info.id;
                }
                Ok(Event::CallState { id, state: CallState::Active }) => {
                    assert_eq!(id, call_id);
                    saw_active = true;
                    break;
                }
                Ok(_) => {}
                Err(e) => panic!("mock went quiet: {e:?}"),
            }
        }
        assert!(saw_active);

        link.tx.send(Command::Hangup(call_id.clone())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(Instant::now() < deadline, "no Terminated after hangup");
            if let Ok(Event::CallState { id, state: CallState::Terminated }) =
                link.rx.recv_timeout(Duration::from_secs(5))
            {
                assert_eq!(id, call_id);
                break;
            }
        }
        link.tx.send(Command::Quit).unwrap();
    }
}
