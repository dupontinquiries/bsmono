//! A read-only survey of what the hardware will do: USB-C ports and their roles, the UDC, `/dev/hidg*`.

use std::fs;
use std::path::Path;

fn read(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// One USB-C port as the kernel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub name: String,
    pub data_role: String,
    pub power_role: String,
    pub mode: String,
    pub partner: bool,
}

/// Parse `sysfs`' bracketed role form, "host [device]" -> "device".
pub fn current_role(s: &str) -> String {
    s.split_whitespace().find_map(|w| w.strip_prefix('[').and_then(|w| w.strip_suffix(']'))).unwrap_or(s).to_string()
}

pub fn ports(root: &Path) -> Vec<Port> {
    let mut v: Vec<Port> = fs::read_dir(root.join("class/typec"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("port") || name.contains('-') {
                return None;
            }
            let d = e.path();
            Some(Port {
                data_role: current_role(&read(d.join("data_role")).unwrap_or_default()),
                power_role: current_role(&read(d.join("power_role")).unwrap_or_default()),
                mode: read(d.join("power_operation_mode")).unwrap_or_default(),
                partner: root.join("class/typec").join(format!("{name}-partner")).exists(),
                name,
            })
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

pub fn udcs(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root.join("class/udc")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    v.sort();
    v
}

pub fn hidg_nodes(dev: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dev).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("hidg")).collect();
    v.sort();
    v
}

/// A plain-text report, and what to do next.
pub fn report(root: &Path, dev: &Path) -> String {
    let (ps, udc, hid) = (ports(root), udcs(root), hidg_nodes(dev));
    let mut s = String::new();
    s.push_str("USB-C ports:\n");
    if ps.is_empty() {
        s.push_str("  (none: no /sys/class/typec)\n");
    }
    for p in &ps {
        s.push_str(&format!("  {}: data={} power={} mode={} partner={}\n", p.name, p.data_role, p.power_role, p.mode, if p.partner { "yes" } else { "no" }));
    }
    s.push_str(&format!("UDC (what a gadget binds to): {}\n", if udc.is_empty() { "none".into() } else { udc.join(", ") }));
    s.push_str(&format!("/dev/hidg*: {}\n", if hid.is_empty() { "none".into() } else { hid.join(", ") }));
    s.push_str("Next: ");
    s.push_str(if !hid.is_empty() {
        "the keyboard exists: `usbkb type \"...\"` or `usbkb gui`.\n"
    } else if udc.is_empty() {
        "no UDC: put the port with the phone into device role first (`usbkb script swap <port>`), then `usbkb script setup`.\n"
    } else {
        "a UDC exists: paste `usbkb script setup` into a root shell.\n"
    });
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bracketed_role_is_the_current_one() {
        assert_eq!(current_role("host [device]"), "device");
        assert_eq!(current_role("[host] device"), "host");
        assert_eq!(current_role("[sink]"), "sink");
        assert_eq!(current_role(""), "");
    }

    #[test]
    fn a_fake_sysfs_is_read_back_and_the_next_step_follows_from_it() {
        let root = std::env::temp_dir().join(format!("usbkb-sys-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let p2 = root.join("class/typec/port2");
        fs::create_dir_all(&p2).unwrap();
        fs::create_dir_all(root.join("class/typec/port2-partner")).unwrap();
        fs::write(p2.join("data_role"), "[host] device\n").unwrap();
        fs::write(p2.join("power_role"), "[source] sink\n").unwrap();
        fs::write(p2.join("power_operation_mode"), "usb_power_delivery\n").unwrap();
        let dev = root.join("dev");
        fs::create_dir_all(&dev).unwrap();
        let ps = ports(&root);
        assert_eq!(ps.len(), 1, "the -partner directory is not a port");
        assert_eq!((ps[0].data_role.as_str(), ps[0].partner), ("host", true));
        let r = report(&root, &dev);
        assert!(r.contains("no UDC") && r.contains("swap"), "{r}");
        fs::create_dir_all(root.join("class/udc/f02280000.usb")).unwrap();
        assert!(report(&root, &dev).contains("paste `usbkb script setup`"));
        fs::write(dev.join("hidg0"), "").unwrap();
        assert!(report(&root, &dev).contains("`usbkb type"));
        let _ = fs::remove_dir_all(&root);
    }
}
