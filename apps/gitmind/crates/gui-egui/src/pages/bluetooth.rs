//! Bluetooth OBEX push — the second way to move a bundle to a device that
//! shares no LAN with this one.
//!
//! `cockpit::bttransfer` has been live in the Qt GUI since before this port
//! began, and the port doc listed it as *"stale Bluetooth/advertise items...
//! if ever needed"*. It is not stale: `gui/src/main.rs` calls `bt_devices()`
//! and `bt_send()` today, and the egui network page has **zero** Bluetooth
//! references. That is a feature the port silently dropped.
//!
//! ## Why this exists at all
//!
//! *"A second way to move a file — typically the repo bundle — to a device
//! that shares no LAN with this one."* The whole fleet story assumes a shared
//! network; Bluetooth is the answer when there isn't one, which on a phone is
//! not a rare case.
//!
//! ## The transfer runs off the UI thread, and that is not optional
//!
//! The Qt side comments *"on the worker thread (transfers are slow)"*. OBEX
//! push over Bluetooth moves a repo bundle at a few hundred KB/s — a 20 MB
//! bundle is most of a minute. Doing that on the paint thread freezes the
//! whole app for the duration, and the user cannot even cancel.
//!
//! ## Three tools, and the argument shapes genuinely differ
//!
//! `bt-obex -p ADDR FILE`, `obexftp -b ADDR -p FILE`, `bluetooth-sendto
//! --device=ADDR FILE`. A single command template would work for none of them,
//! which is why `bt_push_command` returns `Option` per tool rather than
//! formatting a string.

use crate::app::App;

/// A transfer in flight or finished.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum SendState {
    #[default]
    Idle,
    /// Handed to the worker. The UI stays live.
    Sending,
    Done(String),
    Failed(String),
}

impl SendState {
    pub fn is_busy(&self) -> bool { matches!(self, SendState::Sending) }

    /// May a new send start?
    ///
    /// Not while one is running: OBEX push to one device is serial, and a
    /// second attempt either fails obscurely or interleaves with the first.
    pub fn can_start(&self) -> bool { !self.is_busy() }
}

/// Page state, held on `App`.
#[derive(Clone, Default)]
pub struct BluetoothState {
    pub devices: Vec<(String, String)>,
    pub selected: Option<usize>,
    pub file: String,
    pub state: SendState,
    /// Set when scanning failed — kept distinct from "no devices", since a
    /// missing bluetoothctl and an empty pairing list need different advice.
    pub scan_error: Option<String>,
}

impl BluetoothState {
    /// Is the form complete enough to send?
    pub fn ready(&self) -> bool {
        self.selected.is_some() && !self.file.trim().is_empty() && self.state.can_start()
    }

    /// The selected device's address, if any.
    pub fn address(&self) -> Option<&str> {
        self.selected.and_then(|i| self.devices.get(i)).map(|(a, _)| a.as_str())
    }

    /// What to tell the user when there is nothing to send to.
    ///
    /// Three different situations that all look like an empty list, and each
    /// has a different fix — telling somebody to "pair a device" when
    /// bluetoothctl is missing sends them to the wrong settings page.
    pub fn empty_hint(&self) -> Option<&'static str> {
        if let Some(e) = &self.scan_error {
            return Some(if e.contains("No such file") || e.contains("not found") {
                "bluetoothctl is not installed — install bluez to send over Bluetooth."
            } else {
                "Could not list devices. Is the Bluetooth service running?"
            });
        }
        if self.devices.is_empty() {
            return Some("No paired devices. Pair the target device first, \
                         then scan again.");
        }
        None
    }
}

/// Rendered as a section of the Network page rather than its own tab — the Qt
/// GUI puts it there too, and it answers the same question that page is about:
/// these two machines share no LAN.
pub fn section(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Bluetooth send");
    ui.label("Move a bundle to a device that shares no network with this one.");
    ui.add_space(8.0);

    let bt = &mut app.bluetooth;

    ui.horizontal(|ui| {
        if ui.button("Scan for paired devices").clicked() {
            match cockpit::bttransfer::bt_devices() {
                Ok(devs) => {
                    bt.scan_error = None;
                    bt.devices = devs.into_iter().map(|d| (d.addr, d.name)).collect();
                    // A scan invalidates the selection: index 2 in the old list
                    // is a different device in the new one, and sending to the
                    // wrong phone is not recoverable.
                    bt.selected = None;
                }
                Err(e) => {
                    bt.scan_error = Some(e);
                    bt.devices.clear();
                    bt.selected = None;
                }
            }
        }
        if bt.state.is_busy() {
            ui.spinner();
            ui.label("Sending…");
        }
    });

    ui.add_space(6.0);

    if let Some(hint) = bt.empty_hint() {
        ui.label(hint);
    } else {
        // Labels built first: radio_value needs `&mut bt.selected` while
        // iterating `bt.devices` would still be borrowing bt immutably. Two
        // borrows of one struct, and the fix is to stop needing the second —
        // the loop only ever wanted strings.
        let labels: Vec<String> = bt.devices.iter()
            .map(|(addr, name)| if name.is_empty() { addr.clone() }
                                else { format!("{name}  ({addr})") })
            .collect();
        for (i, label) in labels.into_iter().enumerate() {
            ui.radio_value(&mut bt.selected, Some(i), label);
        }
    }

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("File:");
        ui.add(egui::TextEdit::singleline(&mut bt.file).id_salt("bluetooth-bt_file"));
    });

    ui.add_space(8.0);
    // Disabled rather than hidden: a button that vanishes leaves the user
    // hunting for the feature, while a greyed one shows what is missing.
    // Gated on the SHARED task slot, not just our own state: the app runs one
    // background job at a time, and starting a minute-long OBEX push while a
    // deploy is running would make both look stuck.
    let ready = bt.ready() && !app.task.is_running();
    let addr = bt.address().map(str::to_string);
    let file = bt.file.trim().to_string();
    if ui.add_enabled(ready, egui::Button::new("Send over Bluetooth")).clicked() {
        if let Some(addr) = addr {
            app.bluetooth.state = SendState::Sending;
            // Off the paint thread: an OBEX push of a repo bundle is most of a
            // minute, and doing it here freezes the app with no way to cancel.
            app.task.run(move || match cockpit::bttransfer::bt_send(&addr, &file) {
                Ok(msg) => msg,
                Err(e) => format!("failed: {e}"),
            });
        }
    }

    // Drain the shared slot into our own state, so the result survives a tab
    // switch — a transfer that finishes while the user is elsewhere must not
    // silently vanish.
    if app.bluetooth.state.is_busy() && !app.task.is_running() {
        if let Some(out) = app.task.take_result() {
            app.bluetooth.state = if out.starts_with("failed: ") {
                SendState::Failed(out.trim_start_matches("failed: ").to_string())
            } else {
                SendState::Done(out)
            };
        }
    }

    match &app.bluetooth.state {
        SendState::Done(m) => { ui.add_space(6.0); ui.label(format!("Sent: {m}")); }
        SendState::Failed(e) => { ui.add_space(6.0); ui.label(format!("Failed: {e}")); }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(devices: usize) -> BluetoothState {
        BluetoothState {
            devices: (0..devices).map(|i| (format!("AA:BB:{i:02}"), format!("dev{i}"))).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_send_needs_both_a_device_and_a_file() {
        // Either alone would send nothing, or send to nobody.
        let mut s = state_with(2);
        assert!(!s.ready(), "ready with no selection");
        s.selected = Some(0);
        assert!(!s.ready(), "ready with no file");
        s.file = "repo.bundle".into();
        assert!(s.ready());
    }

    #[test]
    fn a_whitespace_filename_is_not_a_file() {
        let mut s = state_with(1);
        s.selected = Some(0);
        s.file = "   ".into();
        assert!(!s.ready());
    }

    #[test]
    fn a_second_send_cannot_start_while_one_is_running() {
        // OBEX push to one device is serial; a second attempt either fails
        // obscurely or interleaves with the first.
        let mut s = state_with(1);
        s.selected = Some(0);
        s.file = "repo.bundle".into();
        s.state = SendState::Sending;
        assert!(!s.ready());
        s.state = SendState::Done("ok".into());
        assert!(s.ready(), "a finished send blocked the next one");
    }

    #[test]
    fn a_failed_send_does_not_block_a_retry() {
        // Bluetooth fails transiently — the peer was asleep, the pairing
        // lapsed — and a retry is the normal response.
        let mut s = state_with(1);
        s.selected = Some(0);
        s.file = "f".into();
        s.state = SendState::Failed("peer refused".into());
        assert!(s.ready());
    }

    #[test]
    fn a_missing_tool_and_an_empty_pairing_list_give_different_advice() {
        // Both look like an empty list, and each has a different fix — telling
        // somebody to "pair a device" when bluetoothctl is missing sends them
        // to the wrong settings page entirely.
        let mut s = BluetoothState::default();
        assert!(s.empty_hint().unwrap().contains("Pair the target device"));

        s.scan_error = Some("No such file or directory".into());
        assert!(s.empty_hint().unwrap().contains("bluez"));

        s.scan_error = Some("org.bluez was not provided".into());
        assert!(s.empty_hint().unwrap().contains("service running"));
    }

    #[test]
    fn a_populated_list_shows_no_hint() {
        let mut s = state_with(3);
        assert_eq!(s.empty_hint(), None);
        // ...but a scan error wins even with a stale list, because those
        // entries are from before the failure and may no longer exist.
        s.scan_error = Some("boom".into());
        assert!(s.empty_hint().is_some());
    }

    #[test]
    fn the_address_follows_the_selection() {
        let mut s = state_with(3);
        s.selected = Some(1);
        assert_eq!(s.address(), Some("AA:BB:01"));
        s.selected = None;
        assert_eq!(s.address(), None);
    }

    #[test]
    fn an_out_of_range_selection_yields_no_address_rather_than_panicking() {
        // Reachable if a scan shortens the list before the click is handled.
        let mut s = state_with(2);
        s.selected = Some(9);
        assert_eq!(s.address(), None);
    }

    #[test]
    fn the_three_tools_take_genuinely_different_arguments() {
        // A single command template would work for none of them, which is why
        // bt_push_command returns Option per tool rather than formatting a
        // string.
        use cockpit::bttransfer::bt_push_command;
        let (p, a) = bt_push_command("bt-obex", "AA:BB", "f").expect("bt-obex");
        assert_eq!(p, "bt-obex");
        assert_eq!(a, vec!["-p", "AA:BB", "f"]);

        let (_, a) = bt_push_command("obexftp", "AA:BB", "f").expect("obexftp");
        assert_eq!(a, vec!["-b", "AA:BB", "-p", "f"]);

        let (_, a) = bt_push_command("bluetooth-sendto", "AA:BB", "f").expect("sendto");
        assert_eq!(a, vec!["--device=AA:BB", "f"]);

        assert!(bt_push_command("scp", "AA:BB", "f").is_none(),
                "an unknown tool must not be guessed at");
    }
}
