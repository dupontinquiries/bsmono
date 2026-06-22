//! WiFi SSID detection for Linux (iwd, wpa_supplicant, NetworkManager, /sys).
//!
//! Rust port of LocationService::readWifiSsid().
//! Returns the connected WiFi network name, or None if unavailable.
//!
//! Priority order (matches the improved C++ version):
//! 1. NetworkManager D-Bus (GNOME/KDE desktops)
//! 2. iwd D-Bus (postmarketOS/Alpine/Arch)
//! 3. wpa_supplicant D-Bus (fallback for many distros)
//! 4. /sys/class/net/<iface>/wireless/ssid (direct kernel interface)
//!
//! This module is Linux-only. On other platforms it always returns None.

/// Try to read the connected WiFi SSID using any available method.
/// Returns `None` if not connected to WiFi or all methods fail.
pub fn read_ssid() -> Option<String> {
    // Method 4: /sys/class/net — works everywhere on Linux without D-Bus.
    // Listed first because it has no external dependencies.
    if let Some(s) = read_ssid_from_sys() { return Some(s); }
    None
    // Methods 1-3 require D-Bus, which we can't easily use from pure Rust
    // without a D-Bus crate dependency. The Qt C++ layer calls these via
    // QDBusInterface; in Rust they would use the `zbus` crate.
    // Add `zbus` to Cargo.toml and implement here when needed.
    //
    // Stub signatures for future implementation:
    // fn read_ssid_networkmanager() -> Option<String> { todo!() }
    // fn read_ssid_iwd()            -> Option<String> { todo!() }
    // fn read_ssid_wpa_supplicant() -> Option<String> { todo!() }
}

/// Read SSID from /sys/class/net/<iface>/wireless/ssid (Linux kernel interface).
fn read_ssid_from_sys() -> Option<String> {
    // Typical wireless interface names on Linux
    let candidates = ["wlan0", "wlp2s0", "wlp3s0", "wlan1", "mlan0"];
    for iface in &candidates {
        let path = format!("/sys/class/net/{}/wireless/ssid", iface);
        if let Ok(data) = std::fs::read(&path) {
            let ssid = String::from_utf8_lossy(&data).trim().to_owned();
            if !ssid.is_empty() { return Some(ssid); }
        }
    }
    // Also try scanning /sys/class/net/ for any wlan* interface
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("wlan") && !name.starts_with("wlp") { continue; }
            let path = format!("/sys/class/net/{}/wireless/ssid", name);
            if let Ok(data) = std::fs::read(&path) {
                let ssid = String::from_utf8_lossy(&data).trim().to_owned();
                if !ssid.is_empty() { return Some(ssid); }
            }
        }
    }
    None
}
