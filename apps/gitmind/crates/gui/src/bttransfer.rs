//! Best-effort Bluetooth (OBEX Object Push) file send for the gitmind GUI.
//!
//! A second way to move a file — typically the repo bundle — to a device that
//! shares no LAN with this one: over Bluetooth. The actual transfer shells out to
//! whichever OBEX push tool the device has (`bt-obex` from bluez-tools, `obexftp`,
//! or GNOME's `bluetooth-sendto`). Only the `bluetoothctl` parsing and the command
//! construction are unit-tested; the transfer itself needs a real radio + a paired,
//! accepting peer, which the sandbox has none of — so `bt_devices`/`bt_send` run
//! on-device. See maintenance/SHARING.md for where this fits the transport seam.

use std::process::Command;

/// A paired / known Bluetooth device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtDevice {
    pub addr: String,
    pub name: String,
}

/// Parse `bluetoothctl devices` output. Each line is `Device AA:BB:CC:DD:EE:FF Name`.
/// Pure — unit-tested.
pub fn parse_bt_devices(out: &str) -> Vec<BtDevice> {
    out.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("Device ")?;
            let (addr, name) = rest.split_once(' ')?;
            let addr = addr.trim();
            // a Bluetooth address is 6 colon-separated hex bytes
            if addr.split(':').count() != 6 {
                return None;
            }
            Some(BtDevice {
                addr: addr.to_string(),
                name: name.trim().to_string(),
            })
        })
        .collect()
}

/// `(program, args)` to push `file` to `addr` for a given OBEX tool. Pure —
/// unit-tested; tool availability is checked separately at send time.
pub fn bt_push_command(tool: &str, addr: &str, file: &str) -> Option<(String, Vec<String>)> {
    match tool {
        // bluez-tools — cleanest headless push
        "bt-obex" => Some(("bt-obex".into(), vec!["-p".into(), addr.into(), file.into()])),
        // obexftp — let it discover the OPP channel
        "obexftp" => Some((
            "obexftp".into(),
            vec!["-b".into(), addr.into(), "-p".into(), file.into()],
        )),
        // GNOME helper (pops its own progress UI)
        "bluetooth-sendto" => Some((
            "bluetooth-sendto".into(),
            vec![format!("--device={addr}"), file.into()],
        )),
        _ => None,
    }
}

/// The OBEX push tools we know how to drive, in preference order.
pub const BT_TOOLS: [&str; 3] = ["bt-obex", "obexftp", "bluetooth-sendto"];

/// List paired / known Bluetooth devices via `bluetoothctl`. On-device.
pub fn bt_devices() -> Result<Vec<BtDevice>, String> {
    let out = Command::new("bluetoothctl")
        .arg("devices")
        .output()
        .map_err(|e| format!("bluetoothctl not available ({e}) — install bluez"))?;
    Ok(parse_bt_devices(&String::from_utf8_lossy(&out.stdout)))
}

/// First OBEX push tool that's actually installed. On-device.
fn detect_bt_tool() -> Option<&'static str> {
    BT_TOOLS.iter().copied().find(|t| {
        Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {t} >/dev/null 2>&1"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Send `file` to Bluetooth device `addr` over OBEX push. The peer must be paired
/// and accepting an incoming file. On-device.
pub fn bt_send(addr: &str, file: &str) -> Result<String, String> {
    let a = addr.trim();
    if a.split(':').count() != 6 {
        return Err(format!("'{a}' is not a Bluetooth address (AA:BB:CC:DD:EE:FF)"));
    }
    if !std::path::Path::new(file).exists() {
        return Err(format!("file not found: {file}"));
    }
    let tool = detect_bt_tool()
        .ok_or("no OBEX push tool found — install bluez-tools (bt-obex) or obexftp")?;
    let (prog, args) = bt_push_command(tool, a, file).ok_or("unsupported OBEX tool")?;
    let st = Command::new(&prog)
        .args(&args)
        .status()
        .map_err(|e| format!("{prog}: {e}"))?;
    if st.success() {
        Ok(format!("sent {file} → {a} via {tool}"))
    } else {
        Err(format!(
            "{tool} failed (exit {:?}) — is {a} paired and set to accept files?",
            st.code()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bluetoothctl_devices_and_skips_noise() {
        let out = "Device AA:BB:CC:DD:EE:FF Pixel Phone\n\
                   Device 11:22:33:44:55:66 fairphone5\n\
                   Controller 00:11:22:33:44:55 host [default]\n\
                   not a device line\n";
        let d = parse_bt_devices(out);
        assert_eq!(d.len(), 2, "two Device lines, controller + junk skipped");
        assert_eq!(d[0], BtDevice { addr: "AA:BB:CC:DD:EE:FF".into(), name: "Pixel Phone".into() });
        assert_eq!(d[1].name, "fairphone5");
    }

    #[test]
    fn push_commands_per_tool() {
        let (p, a) = bt_push_command("bt-obex", "AA:BB:CC:DD:EE:FF", "/tmp/repo.bundle").unwrap();
        assert_eq!(p, "bt-obex");
        assert_eq!(a, vec!["-p", "AA:BB:CC:DD:EE:FF", "/tmp/repo.bundle"]);
        let (p2, a2) = bt_push_command("obexftp", "AA:BB:CC:DD:EE:FF", "/tmp/x").unwrap();
        assert_eq!(p2, "obexftp");
        assert!(a2.contains(&"-p".to_string()) && a2.contains(&"/tmp/x".to_string()));
        assert!(bt_push_command("nope", "AA:BB:CC:DD:EE:FF", "/tmp/x").is_none());
    }

    #[test]
    fn send_rejects_bad_address_and_missing_file_before_any_radio() {
        assert!(bt_send("not-an-addr", "/tmp/x").is_err());
        assert!(bt_send("AA:BB:CC:DD:EE:FF", "/definitely/missing/file").is_err());
    }
}
