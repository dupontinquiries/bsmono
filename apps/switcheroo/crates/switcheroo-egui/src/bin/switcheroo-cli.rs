//! switcheroo-cli — turn a Wemo on or off from a terminal, on the Fairphone or the Studio, with no window.
//!
//!   Wi-Fi Wemos (Mini Smart Plug, Smart Plug, Insight, Light Switch): local SOAP, nothing to pair.
//!     switcheroo-cli wifi discover
//!     switcheroo-cli wifi on|off|status <host[:port]>
//!
//!   WSP100 (HomeKit-over-BLE/Thread): hap-core over btleplug, the same code the egui app runs.
//!     switcheroo-cli ble scan [seconds]
//!     switcheroo-cli ble pair <addr> <setup-code>        # XXX-XX-XXX from the label; one admin controller at a time
//!     switcheroo-cli ble on|off|status|identify|unpair <addr> [--trace]
//!
//! Every BLE command prints hap-core's PDU trace with --trace; an error at pair-setup state 4 is the
//! SRP-convention question HOMEKIT.md names, and the trace is the evidence to settle it with.
use std::time::Duration;

fn usage() -> ! {
    eprintln!("usage:\n  switcheroo-cli wifi discover\n  switcheroo-cli wifi on|off|status <host[:port]>\n  switcheroo-cli ble scan [seconds]\n  switcheroo-cli ble pair <addr> <setup-code>\n  switcheroo-cli ble on|off|status|identify|unpair <addr> [--trace]");
    std::process::exit(2)
}

fn wifi(args: &[String]) -> Result<(), String> {
    use switcheroo_core::wemo_wifi as w;
    match args.first().map(|s| s.as_str()) {
        Some("discover") => {
            let found = w::discover(Duration::from_secs(3));
            if found.is_empty() { println!("no Wi-Fi Wemos answered SSDP (same subnet? multicast allowed?)"); }
            for f in found { println!("{:24} {}:{}  {}", f.friendly_name, f.host, f.port, f.model); }
            Ok(())
        }
        Some(cmd @ ("on" | "off" | "status")) => {
            let target = args.get(1).ok_or("host[:port]")?;
            let (host, port) = match target.rsplit_once(':') { Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()), _ => (target.clone(), w::find_port(target).ok_or_else(|| format!("{target}: no Wemo on ports {:?}", w::PORTS))?) };
            let state = match cmd { "on" => w::set_state(&host, port, true)?, "off" => w::set_state(&host, port, false)?, _ => w::get_state(&host, port)? };
            println!("{} ({}:{}) is {}", w::friendly_name(&host, port).unwrap_or_else(|_| host.clone()), host, port, match state { 0 => "off", 1 => "on", 8 => "on (standby)", n => return Ok(println!("state {n}")) });
            Ok(())
        }
        _ => usage(),
    }
}

#[cfg(feature = "ble")]
fn ble(args: &[String]) -> Result<(), String> {
    use btleplug::api::{Central as _, Manager as _, Peripheral as _, ScanFilter};
    use btleplug::platform::{Manager, Peripheral};
    use switcheroo_core::DeviceId;
    use switcheroo_egui::hap;

    let trace = args.iter().any(|a| a == "--trace");
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let manager = Manager::new().await.map_err(|e| format!("bluetooth: {e} (is bluetoothd running and this user allowed on D-Bus?)"))?;
        let adapter = manager.adapters().await.map_err(|e| e.to_string())?.into_iter().next().ok_or("no Bluetooth adapter")?;
        let find = |addr: &str, secs: u64| {
            let adapter = adapter.clone(); let addr = addr.to_ascii_lowercase();
            async move {
                adapter.start_scan(ScanFilter::default()).await.map_err(|e| e.to_string())?;
                let t0 = std::time::Instant::now();
                let p: Peripheral = loop {
                    let hit = adapter.peripherals().await.map_err(|e| e.to_string())?.into_iter().find(|p| p.address().to_string().to_ascii_lowercase() == addr);
                    if let Some(p) = hit { adapter.stop_scan().await.ok(); break p; }
                    if t0.elapsed() > Duration::from_secs(secs) { adapter.stop_scan().await.ok(); return Err(format!("{addr}: not seen in {secs}s (advertising? in range? already connected to a phone?)")); }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                };
                Ok::<Peripheral, String>(p)
            }
        };
        match args.first().map(|s| s.as_str()) {
            Some("scan") => {
                let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(6);
                adapter.start_scan(ScanFilter::default()).await.map_err(|e| e.to_string())?;
                tokio::time::sleep(Duration::from_secs(secs)).await;
                for p in adapter.peripherals().await.map_err(|e| e.to_string())? {
                    let props = p.properties().await.ok().flatten();
                    let name = props.as_ref().and_then(|q| q.local_name.clone()).unwrap_or_default();
                    let hap = props.as_ref().map(|q| q.manufacturer_data.contains_key(&0x004C)).unwrap_or(false);
                    if !name.is_empty() || hap { println!("{}  {:20} {}", p.address(), name, if hap { "(Apple manufacturer data: HomeKit advert)" } else { "" }); }
                }
                adapter.stop_scan().await.ok();
                Ok(())
            }
            Some("pair") => {
                let addr = args.get(1).ok_or("addr")?; let code = args.get(2).ok_or("setup code")?.clone();
                let p = find(addr, 15).await?;
                let store = hap::Store::open();
                let id = DeviceId::new(p.address().to_string());
                let (_pairing, log) = hap::pair(p, &store, &id, code).await?;
                if trace { for l in &log { eprintln!("{l}"); } }
                println!("paired {addr}; record in {}", store.dir().display());
                Ok(())
            }
            Some(cmd @ ("on" | "off" | "status" | "identify" | "unpair")) => {
                let addr = args.get(1).ok_or("addr")?;
                let store = hap::Store::open();
                let id = DeviceId::new(addr.to_ascii_lowercase());
                let pairing = store.load(&id).or_else(|| store.load(&DeviceId::new(addr.clone()))).ok_or_else(|| format!("no pairing record for {addr}; run `ble pair` first"))?;
                let p = find(addr, 15).await?;
                match cmd {
                    "on" | "off" => {
                        let (actual, log) = hap::set_power(p, pairing, cmd == "on").await?;
                        if trace { for l in &log { eprintln!("{l}"); } }
                        println!("{addr} reports {}", if actual { "on" } else { "off" });
                    }
                    "status" => {
                        let (caps, power, in_use, log) = hap::inspect(p, pairing).await?;
                        if trace { for l in &log { eprintln!("{l}"); } }
                        println!("{addr}: power {:?}, in use {:?}, capabilities {:?}", power, in_use, caps);
                    }
                    "identify" => { let log = hap::identify(p, pairing).await?; if trace { for l in &log { eprintln!("{l}"); } } println!("{addr}: identify sent (it should blink)"); }
                    _ => { let log = hap::remove_pairing(p, pairing).await?; if trace { for l in &log { eprintln!("{l}"); } } store.forget(&id); println!("{addr}: pairing removed on both sides"); }
                }
                Ok(())
            }
            _ => usage(),
        }
    })
}

#[cfg(not(feature = "ble"))]
fn ble(_args: &[String]) -> Result<(), String> { Err("built without the `ble` feature".into()) }

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(|s| s.as_str()) {
        Some("wifi") => wifi(&args[1..]),
        Some("ble") => ble(&args[1..]),
        _ => usage(),
    };
    if let Err(e) = r { eprintln!("switcheroo-cli: {e}"); std::process::exit(1); }
}
