//! The real radio, behind [`switcheroo_core::Backend`].
//!
//! btleplug is async (it wraps BlueZ's D-Bus API on Linux). The core's
//! Backend trait is deliberately sync and dyn-compatible, so the async
//! lives entirely in here: one tokio runtime on one owned thread, two
//! std mpsc channels across the boundary. The UI thread never awaits and
//! never sees a `Future`.
//!
//! **State of this file, stated plainly:** discovery is real and complete.
//! Control is real only for [`Kind::GenericGatt`]. HomeKit control is not
//! implemented and this file does not pretend otherwise — see
//! HOMEKIT.md, and since hap-core landed, `crate::hap`. None of this has been run
//! against a physical device by its author; it is written from btleplug's
//! documented API and needs a hardware pass.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use switcheroo_core::{classify, Backend, Command, DeviceId, Event, Kind, Power};

use btleplug::api::{
    Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::Manager;
use futures::StreamExt;

/// Assigned 16-bit UUID 0x2A57 "Digital Output" inside the 0x1815
/// Automation IO service — the characteristic the simple relay plugs
/// expose. One byte: 0 off, 1 on.
///
/// This is the assigned-numbers answer, not a vendor's answer. A specific
/// plug may well use a proprietary characteristic instead, in which case
/// writing here fails cleanly (the characteristic is simply absent) and
/// the row shows the error rather than silently doing nothing.
const DIGITAL_OUTPUT: uuid::Uuid = uuid::Uuid::from_u128(0x00002a57_0000_1000_8000_00805f9b34fb);

pub struct BleBackend {
    tx: Sender<Command>,
    rx: Receiver<Event>,
    adapter: String,
}

impl BleBackend {
    /// Spin up the radio thread. Returns `Err` with a human-readable reason
    /// if there is no usable adapter — the caller falls back to the demo
    /// backend and says so in the status bar, rather than showing an empty
    /// list that looks like "no devices nearby".
    pub fn start() -> Result<BleBackend, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<String, String>>();

        std::thread::Builder::new()
            .name("switcheroo-ble".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(format!("tokio runtime: {e}")));
                        return;
                    }
                };
                rt.block_on(radio_loop(cmd_rx, ev_tx, ready_tx));
            })
            .map_err(|e| format!("spawn radio thread: {e}"))?;

        // Block briefly for the adapter probe only. If BlueZ is slow or
        // absent we want to know now, at construction, not three frames
        // into the UI.
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(adapter)) => Ok(BleBackend {
                tx: cmd_tx,
                rx: ev_rx,
                adapter,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("bluetooth adapter did not respond within 5s".into()),
        }
    }
}

impl Backend for BleBackend {
    fn submit(&mut self, cmd: Command) {
        // A dead channel means the radio thread panicked or exited. Swallow
        // rather than panic the UI: poll() will stop producing events and
        // the rows go stale, which is a survivable degradation.
        let _ = self.tx.send(cmd);
    }

    fn poll(&mut self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    fn describe(&self) -> String {
        format!("btleplug · {}", self.adapter)
    }
}

async fn radio_loop(
    cmd_rx: Receiver<Command>,
    ev_tx: Sender<Event>,
    ready_tx: Sender<Result<String, String>>,
) {
    let manager = match Manager::new().await {
        Ok(m) => m,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("bluetooth manager: {e}")));
            return;
        }
    };
    let adapters = match manager.adapters().await {
        Ok(a) => a,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("list adapters: {e}")));
            return;
        }
    };
    let Some(central) = adapters.into_iter().next() else {
        let _ = ready_tx.send(Err(
            "no bluetooth adapter found (is the radio enabled?)".into()
        ));
        return;
    };
    let name = central
        .adapter_info()
        .await
        .unwrap_or_else(|_| "adapter".into());
    let _ = ready_tx.send(Ok(name));
    let store = crate::hap::Store::open();

    let mut events = match central.events().await {
        Ok(e) => e,
        Err(e) => {
            let _ = ev_tx.send(Event::Failed {
                id: None,
                message: format!("event stream: {e}"),
            });
            return;
        }
    };

    let mut gsn = hap_core::advert::GsnTracker::new();
    loop {
        // Drain UI commands without blocking the event stream.
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                Command::StartScan => {
                    match central.start_scan(ScanFilter::default()).await {
                        Ok(()) => {
                            let _ = ev_tx.send(Event::ScanStarted);
                        }
                        Err(e) => {
                            let _ = ev_tx.send(Event::Failed {
                                id: None,
                                message: format!("start scan: {e}"),
                            });
                        }
                    }
                }
                Command::StopScan => {
                    let _ = central.stop_scan().await;
                    let _ = ev_tx.send(Event::ScanStopped);
                }
                Command::Refresh(id) => {
                    let r = if kind_of(&central, &id).await == Kind::HomeKit {
                        hap_read_power(&central, &store, &id).await.map(|(p, trace)| {
                            for l in trace { let _ = ev_tx.send(Event::Failed { id: None, message: format!("hap {l}") }); }
                            p
                        })
                    } else {
                        read_power(&central, &id).await
                    };
                    let _ = ev_tx.send(match r {
                        Ok(p) => Event::PowerIs { id, power: p },
                        Err(m) => Event::Failed {
                            id: Some(id),
                            message: m,
                        },
                    });
                }
                Command::SetPower { id, on } => {
                    let r = if kind_of(&central, &id).await == Kind::HomeKit {
                        hap_write_power(&central, &store, &id, on).await.map(|trace| {
                            for l in trace { let _ = ev_tx.send(Event::Failed { id: None, message: format!("hap {l}") }); }
                        })
                    } else {
                        write_power(&central, &id, on).await
                    };
                    let _ = ev_tx.send(match r {
                        Ok(()) => Event::PowerIs {
                            id,
                            power: if on { Power::On } else { Power::Off },
                        },
                        Err(m) => Event::Failed {
                            id: Some(id),
                            message: m,
                        },
                    });
                }
                Command::Pair { id, setup_code } => {
                    let r = match find(&central, &id).await {
                        Ok(p) => crate::hap::pair(p, &store, &id, setup_code).await,
                        Err(e) => Err(e),
                    };
                    let _ = ev_tx.send(match r {
                        Ok((pairing, trace)) => {
                            for l in trace { let _ = ev_tx.send(Event::Failed { id: None, message: format!("hap {l}") }); }
                            let _ = ev_tx.send(Event::Failed { id: None, message: format!("paired with accessory {} (record in {})", pairing.accessory_id, store.dir().display()) });
                            Event::Paired { id }
                        }
                        Err(message) => Event::PairingFailed { id, message },
                    });
                }
                Command::Unpair(id) => {
                    // TELL THE ACCESSORY FIRST. Deleting the record alone left the plug
                    // paired to a controller that no longer exists: still claimed, still
                    // refusing pair-setup, and only a factory reset gets it back — the
                    // exact cost HOMEKIT.md's dialog warns about, arrived at by pressing
                    // a button labelled "forget this pairing".
                    //
                    // A failure here leaves BOTH sides paired, which the user can retry
                    // from; the other order cannot be recovered from at all.
                    let told = match store.load(&id) {
                        Some(pairing) => match find(&central, &id).await {
                            Ok(p) => crate::hap::remove_pairing(p, pairing).await,
                            Err(e) => Err(e),
                        },
                        // No record: nothing to tell it, and forgetting is a no-op that
                        // should still leave the row unpaired.
                        None => Ok(Vec::new()),
                    };
                    match told {
                        Ok(trace) => {
                            for l in trace { let _ = ev_tx.send(Event::Failed { id: None, message: format!("hap {l}") }); }
                            store.forget(&id);
                            let _ = ev_tx.send(Event::Unpaired { id });
                        }
                        Err(message) => {
                            // The record is KEPT. Saying "unpaired" here would lose the
                            // only keys that can undo the pairing.
                            let _ = ev_tx.send(Event::Failed {
                                id: Some(id),
                                message: format!("{message} — the pairing was kept; the accessory still holds it"),
                            });
                        }
                    }
                }
                // ONE SESSION, three answers: what it is, whether it is on, whether
                // anything is plugged in. See `hap::inspect` — each pair-verify is a
                // Curve25519 exchange and two signature round trips over BLE, so asking
                // separately turns opening a row into four handshakes.
                Command::Inspect(id) => {
                    let r = match (store.load(&id), find(&central, &id).await) {
                        (Some(pairing), Ok(p)) => crate::hap::inspect(p, pairing).await,
                        (None, _) => Err("no pairing for this accessory".to_string()),
                        (_, Err(e)) => Err(e),
                    };
                    match r {
                        Ok((caps, power, in_use, trace)) => {
                            for l in trace { let _ = ev_tx.send(Event::Failed { id: None, message: format!("hap {l}") }); }
                            let _ = ev_tx.send(Event::Abilities {
                                id: id.clone(),
                                describes: caps.describe(),
                                can_identify: caps.identifiable(),
                                in_use,
                            });
                            // The state came free with the same session; sending it means
                            // the row does not need a second round trip to stop saying "—".
                            if power != Power::Unknown {
                                let _ = ev_tx.send(Event::PowerIs { id, power });
                            }
                        }
                        Err(message) => { let _ = ev_tx.send(Event::Failed { id: Some(id), message }); }
                    }
                }
                Command::Identify(id) => {
                    let r = match (store.load(&id), find(&central, &id).await) {
                        (Some(pairing), Ok(p)) => crate::hap::identify(p, pairing).await,
                        (None, _) => Err("no pairing for this accessory".to_string()),
                        (_, Err(e)) => Err(e),
                    };
                    if let Err(message) = r {
                        let _ = ev_tx.send(Event::Failed { id: Some(id), message });
                    }
                }
            }
        }

        // Then take one radio event, with a timeout so command draining
        // stays responsive even when nothing is advertising.
        match tokio::time::timeout(Duration::from_millis(200), events.next()).await {
            Ok(Some(CentralEvent::DeviceDiscovered(pid)))
            | Ok(Some(CentralEvent::DeviceUpdated(pid))) => {
                if let Ok(p) = central.peripheral(&pid).await {
                    if let Ok(Some(props)) = p.properties().await {
                        let uuids: Vec<String> =
                            props.services.iter().map(|u| u.to_string()).collect();
                        let id = DeviceId::new(props.address.to_string());
                        let kind = classify(&uuids);
                        let _ = ev_tx.send(Event::Seen {
                            id: id.clone(),
                            name: props.local_name.unwrap_or_default(),
                            kind,
                            rssi: props.rssi,
                        });
                        if kind == Kind::HomeKit && store.has(&id) {
                            let _ = ev_tx.send(Event::Paired { id: id.clone() });
                        }
                        // THE ADVERT ITSELF. A HAP accessory broadcasts a Global State
                        // Number and bumps it whenever a broadcast-enabled characteristic
                        // changes — so a plug switched at the wall, by the Home app or by a
                        // schedule announces it to anyone in radio range, with no pairing,
                        // no connection and no key. Until now switcheroo learned a plug's
                        // state only by connecting, so it showed the old one until someone
                        // pressed ↻.
                        //
                        // Keyed by the advert's HAP device id, not the BLE address: an
                        // accessory that randomises its address is still the same accessory,
                        // and keying on the address would read as a new one on every
                        // rotation.
                        for (company, body) in &props.manufacturer_data {
                            match hap_core::advert::parse_keyed(*company, body) {
                                hap_core::advert::Parsed::Regulatory(a) => {
                                    let change = gsn.observe(&a);
                                    if matches!(
                                        change,
                                        hap_core::advert::Change::StateChanged
                                            | hap_core::advert::Change::Reconfigured
                                    ) {
                                        let _ = ev_tx.send(Event::AccessoryChanged {
                                            id: id.clone(),
                                            reconfigured: change == hap_core::advert::Change::Reconfigured,
                                        });
                                    }
                                }
                                // An encrypted notification means an event IS being
                                // broadcast and we lack the Broadcast Encryption Key to
                                // read it. Treated as "something changed" — which is all
                                // the GSN would have told us anyway — rather than ignored,
                                // because ignoring it is how a controller quietly stops
                                // noticing changes.
                                hap_core::advert::Parsed::EncryptedNotification { .. } => {
                                    let _ = ev_tx.send(Event::AccessoryChanged {
                                        id: id.clone(),
                                        reconfigured: false,
                                    });
                                }
                                hap_core::advert::Parsed::NotHap => {}
                            }
                        }
                    }
                }
            }
            Ok(Some(_)) => {}
            // Stream ended — the adapter went away (rfkill, USB unplug).
            Ok(None) => {
                let _ = ev_tx.send(Event::Failed {
                    id: None,
                    message: "bluetooth adapter disappeared".into(),
                });
                return;
            }
            Err(_) => {} // timeout: loop round and check commands again
        }
    }
}

/// Find a peripheral by address string. btleplug indexes by its own
/// PeripheralId, so this is a scan of the known set rather than a lookup.
async fn find(
    central: &btleplug::platform::Adapter,
    id: &DeviceId,
) -> Result<btleplug::platform::Peripheral, String> {
    let peers = central
        .peripherals()
        .await
        .map_err(|e| format!("list peripherals: {e}"))?;
    for p in peers {
        if let Ok(Some(props)) = p.properties().await {
            if props.address.to_string() == id.as_str() {
                return Ok(p);
            }
        }
    }
    Err("device is no longer in range".into())
}

/// Connect, discover services, and refuse early if this is a HomeKit
/// accessory. The refusal is the point: without it a HomeKit write fails
/// with an opaque GATT error and the user has no idea their plug needs
/// pairing that this app cannot do.
async fn open(
    central: &btleplug::platform::Adapter,
    id: &DeviceId,
) -> Result<btleplug::platform::Peripheral, String> {
    let p = find(central, id).await?;
    if !p.is_connected().await.unwrap_or(false) {
        p.connect().await.map_err(|e| format!("connect: {e}"))?;
    }
    p.discover_services()
        .await
        .map_err(|e| format!("discover services: {e}"))?;
    Ok(p)
}

/// What the advertisement says this device is, from the adapter's cache.
/// Decides whether a power command goes to the plain GATT path or to HAP.
async fn kind_of(central: &btleplug::platform::Adapter, id: &DeviceId) -> Kind {
    if let Ok(p) = find(central, id).await {
        if let Ok(Some(props)) = p.properties().await {
            let uuids: Vec<String> = props.services.iter().map(|u| u.to_string()).collect();
            return classify(&uuids);
        }
    }
    Kind::Unknown
}

/// The HAP path for a power write: needs a stored pairing, else says so.
async fn hap_write_power(
    central: &btleplug::platform::Adapter,
    store: &crate::hap::Store,
    id: &DeviceId,
    on: bool,
) -> Result<Vec<String>, String> {
    let pairing = store.load(id).ok_or_else(|| "HomeKit accessory: pair it first (8-digit setup code on the plug)".to_string())?;
    let p = find(central, id).await?;
    crate::hap::set_power(p, pairing, on).await
}

async fn hap_read_power(
    central: &btleplug::platform::Adapter,
    store: &crate::hap::Store,
    id: &DeviceId,
) -> Result<(Power, Vec<String>), String> {
    let pairing = store.load(id).ok_or_else(|| "HomeKit accessory: pair it first (8-digit setup code on the plug)".to_string())?;
    let p = find(central, id).await?;
    crate::hap::read_power(p, pairing).await
}

async fn write_power(
    central: &btleplug::platform::Adapter,
    id: &DeviceId,
    on: bool,
) -> Result<(), String> {
    let p = open(central, id).await?;
    let ch = p
        .characteristics()
        .into_iter()
        .find(|c| c.uuid == DIGITAL_OUTPUT)
        .ok_or_else(|| {
            "no Automation IO digital-output characteristic (0x2A57) on this device".to_string()
        })?;
    p.write(&ch, &[u8::from(on)], WriteType::WithResponse)
        .await
        .map_err(|e| format!("write: {e}"))
}

async fn read_power(
    central: &btleplug::platform::Adapter,
    id: &DeviceId,
) -> Result<Power, String> {
    let p = open(central, id).await?;
    let ch = p
        .characteristics()
        .into_iter()
        .find(|c| c.uuid == DIGITAL_OUTPUT)
        .ok_or_else(|| {
            "no Automation IO digital-output characteristic (0x2A57) on this device".to_string()
        })?;
    let v = p.read(&ch).await.map_err(|e| format!("read: {e}"))?;
    Ok(match v.first() {
        Some(0) => Power::Off,
        Some(_) => Power::On,
        None => Power::Unknown,
    })
}
