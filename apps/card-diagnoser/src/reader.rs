//! reader — the one part of this app that touches real hardware: a PC/SC
//! service and whatever contactless reader is plugged into it.
//!
//! ## Scope, stated plainly
//!
//! This does exactly two things once a card is present: read its ATR, and
//! send the standard `GET UID` pseudo-APDU (`nfc_kit::GET_UID_APDU`).
//! Nothing here attempts a card-specific authenticated read (a MIFARE
//! Classic key, a DESFire `GetCardUID` past the randomized layer, an EMV
//! application select) — that's real, separate, card-family-specific work,
//! deliberately out of scope for a tool whose whole point is "identify
//! what I'm holding", not "fully interrogate it".
//!
//! ## Threading, same pattern as `pty_host.rs`/`bt_send.rs` elsewhere in
//! this fleet
//!
//! `pcsc::Context::get_status_change` blocks until the card/reader state
//! actually changes — wrong to call from an egui redraw loop. One spawned
//! thread polls it in a loop and writes results into a shared `Mutex`;
//! the UI thread polls that `Mutex` once per frame, same shape as every
//! other real-hardware integration this session has built.
//!
//! ## Verification ceiling
//!
//! No PC/SC daemon or reader exists in a build sandbox. This type-checks
//! against the real `pcsc` 2.x API (confirmed against that crate's own
//! source, not guessed) and handles "no service running"/"no reader
//! attached" as ordinary, expected `ReaderState`s — but the actual
//! card-detected → ATR-read → classify round trip has never run against
//! real hardware.

use nfc_kit::CardProfile;
use pcsc::{Context, Protocols, ReaderState, Scope, ShareMode, State, PNP_NOTIFICATION};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum ReaderStatus {
    /// No PC/SC service reachable at all (`pcscd` not running, or not
    /// installed) — distinct from "service running, no reader plugged in",
    /// since the fix for each is different and the UI should say which.
    NoService(String),
    ServiceRunningNoReader,
    WaitingForCard { reader_name: String },
    CardPresent { reader_name: String, profile: CardProfile },
    /// The card was there but something went wrong reading it (removed
    /// mid-transaction, an unsupported protocol) — a specific, shown-once
    /// error, not silently falling back to `WaitingForCard`.
    CardError { reader_name: String, message: String },
}

/// Spawns the polling thread and returns shared state the UI reads every
/// frame. `wake` is called whenever the status changes so an egui app
/// using this can `request_repaint` — otherwise a card tap could sit
/// unseen until the next unrelated redraw.
pub fn spawn(wake: Waker) -> Arc<Mutex<ReaderStatus>> {
    let status = Arc::new(Mutex::new(ReaderStatus::NoService("starting…".to_string())));
    let status_bg = Arc::clone(&status);

    std::thread::spawn(move || run(status_bg, wake));

    status
}

fn set(status: &Arc<Mutex<ReaderStatus>>, wake: &Waker, new: ReaderStatus) {
    let changed = status.lock().map(|s| *s != new).unwrap_or(true);
    if changed {
        *status.lock().unwrap() = new;
        wake();
    }
}

fn run(status: Arc<Mutex<ReaderStatus>>, wake: Waker) {
    loop {
        let ctx = match Context::establish(Scope::User) {
            Ok(c) => c,
            Err(e) => {
                set(&status, &wake, ReaderStatus::NoService(e.to_string()));
                std::thread::sleep(Duration::from_secs(2)); // retry -- pcscd starting up after this thread does is a real, common race
                continue;
            }
        };

        let mut buf = [0u8; 2048];
        let readers: Vec<std::ffi::CString> = match ctx.list_readers(&mut buf) {
            Ok(names) => names.map(|n| n.to_owned()).collect(),
            Err(_) => vec![],
        };

        if readers.is_empty() {
            set(&status, &wake, ReaderStatus::ServiceRunningNoReader);
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }

        // Watch every reader, plus the PnP pseudo-reader so a reader
        // plugged in AFTER this loop started is noticed rather than
        // requiring a restart.
        let mut reader_states: Vec<ReaderState> = readers
            .iter()
            .map(|r| ReaderState::new(r.clone(), State::UNAWARE))
            .chain(std::iter::once(ReaderState::new(PNP_NOTIFICATION().to_owned(), State::UNAWARE)))
            .collect();

        set(&status, &wake, ReaderStatus::WaitingForCard { reader_name: readers[0].to_string_lossy().into_owned() });

        loop {
            // Blocks until something changes or 2s passes -- the timeout,
            // not just event-driven wakeup, is what lets this loop notice
            // a NEW reader appearing via the PnP entry without a separate
            // poll path for that specifically.
            if ctx.get_status_change(Duration::from_secs(2), &mut reader_states).is_err() {
                break; // service likely went away -- re-establish from the top
            }
            for rs in reader_states.iter_mut() {
                rs.sync_current_state();
            }
            if reader_states.iter().any(|rs| rs.name() == PNP_NOTIFICATION() && rs.event_state().contains(State::CHANGED)) {
                break; // reader list may have changed -- re-enumerate from the top
            }

            for rs in &reader_states {
                if rs.name() == PNP_NOTIFICATION() {
                    continue;
                }
                let name = rs.name().to_string_lossy().into_owned();
                if rs.event_state().contains(State::PRESENT) {
                    match read_card(&ctx, rs.name()) {
                        Ok(profile) => set(&status, &wake, ReaderStatus::CardPresent { reader_name: name, profile }),
                        Err(e) => set(&status, &wake, ReaderStatus::CardError { reader_name: name, message: e }),
                    }
                } else {
                    set(&status, &wake, ReaderStatus::WaitingForCard { reader_name: name });
                }
            }
        }
    }
}

fn read_card(ctx: &Context, reader: &std::ffi::CStr) -> Result<CardProfile, String> {
    let card = ctx
        .connect(reader, ShareMode::Shared, Protocols::ANY)
        .map_err(|e| format!("connect: {e}"))?;

    let mut atr_buf = [0u8; 64];
    let mut names_buf = [0u8; 64];
    let status = card.status2(&mut names_buf, &mut atr_buf).map_err(|e| format!("status: {e}"))?;
    let atr = status.atr().to_vec();

    let mut recv = [0u8; 32];
    let uid = card.transmit(&nfc_kit::GET_UID_APDU, &mut recv).ok().map(|bytes| {
        // A successful GET UID response ends with a 2-byte SW (usually
        // 90 00); the UID itself is everything before that.
        if bytes.len() >= 2 { bytes[..bytes.len() - 2].to_vec() } else { bytes.to_vec() }
    });

    Ok(nfc_kit::classify(&atr, uid.as_deref()))
}
