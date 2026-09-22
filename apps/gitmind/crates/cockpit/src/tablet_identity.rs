//! tablet_identity: WHICH tablet is at an address.
//!
//! Every reMarkable answers on 10.11.99.1 over USB. Trust keyed by address
//! breaks the moment a second tablet is plugged in: an rM2 after a Paper Pro
//! produced OpenSSH's "REMOTE HOST IDENTIFICATION HAS CHANGED" (a different
//! device, not an interception), and the hardware surveys of both were
//! stored under the same key.
//!
//! Two sources, on either side of the cable:
//!
//! * **This machine, before any SSH**: the route to the tablet's address
//!   leaves through a USB network interface; sysfs names the USB device
//!   behind it (vendor/product ids, serial string, product name) and the
//!   interface MAC. That identifies the tablet before a trust decision is
//!   made, so host keys are filed under a per-DEVICE alias
//!   (`HostKeyAlias=gitmind-<id>`) in gitmind's own known_hosts; the user's
//!   `~/.ssh/known_hosts` is not touched. Wi-Fi addresses are per-device
//!   already and keep plain host-key checking.
//! * **The tablet, once connected**: a gitmind cookie (a random id written
//!   under /home at first contact; /home survives reboots and OS updates,
//!   and a factory reset wipes it together with the host key: a new device
//!   in both senses), the SoC's unique serial, the device-tree model.
//!
//! UNVERIFIED on real hardware: whether a reMarkable's USB gadget reports a
//! stable serial string. Without one the alias falls back to the interface
//! MAC, which some gadgets randomise per boot; the finding for a changed
//! host key then says so.

use std::path::{Path, PathBuf};

/// The USB device behind a network interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbLink {
    pub iface: String,
    pub vendor: String,
    pub product: String,
    pub serial: Option<String>,
    pub manufacturer: Option<String>,
    pub product_name: Option<String>,
    pub mac: Option<String>,
}

impl UsbLink {
    /// The stable part: vendor, product and serial (or, lacking a serial,
    /// the interface MAC, which may not be stable).
    pub fn key(&self) -> String {
        let id = self.serial.clone().or_else(|| self.mac.clone().map(|m| format!("mac-{m}"))).unwrap_or_else(|| "unknown".into());
        format!("{}:{}:{}", self.vendor, self.product, id)
    }

    /// `HostKeyAlias` value: the key, restricted to characters ssh and a
    /// file name are both happy with, and LOWERCASE: OpenSSH lowercases host
    /// names for known_hosts lookups, so a mixed-case alias (real serials
    /// look like `RM110-...`) never matched once the entry was hashed.
    pub fn alias(&self) -> String {
        let k: String = self.key().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '-' }).collect();
        format!("gitmind-{k}")
    }

    pub fn has_serial(&self) -> bool {
        self.serial.is_some()
    }

    pub fn describe(&self) -> String {
        format!("{} ({}:{}) serial {} on {}",
            self.product_name.as_deref().unwrap_or("USB device"), self.vendor, self.product,
            self.serial.as_deref().unwrap_or("none (identified by MAC)"), self.iface)
    }
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The USB device behind `iface`, read from sysfs under `sys` (normally
/// `/sys`; tests pass a fake tree). None if the interface is not USB.
pub fn usb_link(iface: &str, sys: &Path) -> Option<UsbLink> {
    let dev = std::fs::canonicalize(sys.join("class/net").join(iface).join("device")).ok()?;
    let sys = std::fs::canonicalize(sys).ok()?;
    // The link points at the USB INTERFACE (1-1:1.0); its parent chain
    // reaches the device (1-1), the first ancestor with idVendor.
    let mut d: Option<&Path> = Some(&dev);
    while let Some(p) = d {
        if !p.starts_with(&sys) {
            return None;
        }
        if p.join("idVendor").is_file() {
            return Some(UsbLink {
                iface: iface.to_string(),
                vendor: read(&p.join("idVendor"))?,
                product: read(&p.join("idProduct")).unwrap_or_default(),
                serial: read(&p.join("serial")),
                manufacturer: read(&p.join("manufacturer")),
                product_name: read(&p.join("product")),
                mac: read(&sys.join("class/net").join(iface).join("address")),
            });
        }
        d = p.parent();
    }
    None
}

/// The interface `ip route get <addr>` says traffic to `addr` leaves by.
pub fn route_iface(addr: &str) -> Option<String> {
    if !addr.chars().all(|c| c.is_ascii_hexdigit() || c == '.' || c == ':') {
        return None; // only literal addresses
    }
    let out = std::process::Command::new("ip").args(["route", "get", addr]).output().ok()?;
    parse_route_dev(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse_route_dev(route: &str) -> Option<String> {
    let mut it = route.split_whitespace();
    while let Some(w) = it.next() {
        if w == "dev" {
            return it.next().map(str::to_string);
        }
    }
    None
}

/// The USB device on the way to `addr`, if the route is over USB.
pub fn usb_link_for(addr: &str) -> Option<UsbLink> {
    usb_link(&route_iface(addr)?, Path::new("/sys"))
}

/// gitmind's own known_hosts for tablets (never the user's).
pub fn known_hosts_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("gitmind/tablet_known_hosts")
}

/// The ssh options for a tablet reached over `link`.
pub fn ssh_opts(link: &UsbLink) -> Vec<String> {
    vec![
        "-o".into(), format!("HostKeyAlias={}", link.alias()),
        "-o".into(), format!("UserKnownHostsFile={}", known_hosts_path().display()),
        // Readable entries in gitmind's own file (systems often default to
        // hashing, which made plain-text lookups miss ssh's own entries).
        "-o".into(), "HashKnownHosts=no".into(),
    ]
}

/// Is a key on file for `alias`? `ssh-keygen -F` matches hashed and plain
/// entries alike.
fn known(alias: &str, file: &Path) -> bool {
    file.is_file() && std::process::Command::new("ssh-keygen").arg("-F").arg(alias).arg("-f").arg(file)
        .output().map(|o| o.status.success() && !o.stdout.is_empty()).unwrap_or(false)
}

/// Before contacting `host`: if it is reached over USB, register the
/// per-device alias with `deploy` (every ssh/scp/rsync to `host` uses it);
/// otherwise clear any earlier registration. Returns the link, if any.
pub fn prepare(host: &str) -> Option<UsbLink> {
    match usb_link_for(host) {
        Some(link) => {
            if let Some(dir) = known_hosts_path().parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            crate::deploy::set_host_opts(host, ssh_opts(&link));
            Some(link)
        }
        None => {
            crate::deploy::set_host_opts(host, vec![]);
            None
        }
    }
}

/// Rewrite scanned `known_hosts` lines (`<host> <type> <key>`) to be filed
/// under `alias`.
pub fn alias_lines(scanned: &str, alias: &str) -> String {
    scanned.lines()
        .filter_map(|l| l.split_once(' ').map(|(_, rest)| format!("{alias} {rest}")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pin the key of the tablet on `host` under its per-device alias (before
/// any password is sent: the same first-contact protection as
/// `deploy::trust_host`, per DEVICE instead of per address).
pub fn trust(host: &str, link: &UsbLink) -> Result<String, String> {
    let file = known_hosts_path();
    let alias = link.alias();
    if known(&alias, &file) {
        return Ok(format!("{} is already trusted", link.describe()));
    }
    let have = std::fs::read_to_string(&file).unwrap_or_default();
    let scanned = crate::deploy::fetch_peer_key(host)?;
    let lines = alias_lines(&scanned, &alias);
    if lines.trim().is_empty() {
        return Err(format!("no SSH key returned for {host}"));
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let mut out = have;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&lines);
    out.push('\n');
    std::fs::write(&file, out).map_err(|e| e.to_string())?;
    Ok(format!("trusted {} (key filed per device, not per address)", link.describe()))
}

/// Forget the key pinned for this device (after its OS update or reset).
pub fn untrust(link: &UsbLink) -> Result<String, String> {
    let file = known_hosts_path();
    let alias = link.alias();
    if !known(&alias, &file) {
        return Ok(format!("no host key on file for {}", link.describe()));
    }
    // ssh-keygen -R removes hashed entries too (a plain-text filter did not).
    let o = std::process::Command::new("ssh-keygen").arg("-R").arg(&alias).arg("-f").arg(&file).output()
        .map_err(|e| format!("ssh-keygen: {e}"))?;
    if !o.status.success() {
        return Err(format!("ssh-keygen -R failed: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let _ = std::fs::remove_file(file.with_extension("old")); // ssh-keygen's backup
    Ok(format!("forgot the host key of {}", link.describe()))
}

/// The key a device's stored data (hardware survey, ...) is filed under:
/// the USB identity when the tablet is on a cable, else the address.
pub fn device_key(host: &str) -> String {
    usb_link_for(host).map(|l| l.alias()).unwrap_or_else(|| host.to_string())
}

/// Where gitmind's cookie lives on a tablet (persistent tree).
pub const COOKIE: &str = "/home/root/.local/share/gitmind/device-id";

/// Create the cookie if absent and print the tablet's identity.
pub fn identity_cmd() -> String {
    format!(
        "f={c}; if [ ! -s \"$f\" ]; then mkdir -p \"$(dirname \"$f\")\"; od -An -tx1 -N16 /dev/urandom | tr -d ' \\n' > \"$f\"; fi; \
         echo \"device_id=$(cat \"$f\")\"; \
         s=$(cat /sys/devices/soc0/serial_number 2>/dev/null); [ -n \"$s\" ] && echo \"soc_serial=$s\"; \
         m=$(tr -d '\\0' < /proc/device-tree/model 2>/dev/null); [ -n \"$m\" ] && echo \"dt_model=$m\"; \
         echo \"hostname=$(hostname)\"; true",
        c = crate::deploy::shell_quote(COOKIE)
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabletIdentity {
    pub device_id: String,
    pub soc_serial: Option<String>,
    pub dt_model: Option<String>,
    pub hostname: Option<String>,
}

pub fn parse_identity(text: &str) -> TabletIdentity {
    let mut t = TabletIdentity::default();
    for l in text.lines() {
        match l.trim().split_once('=') {
            Some(("device_id", v)) => t.device_id = v.to_string(),
            Some(("soc_serial", v)) if !v.is_empty() => t.soc_serial = Some(v.to_string()),
            Some(("dt_model", v)) if !v.is_empty() => t.dt_model = Some(v.to_string()),
            Some(("hostname", v)) if !v.is_empty() => t.hostname = Some(v.to_string()),
            _ => {}
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sysfs tree shaped like the kernel's for a USB network gadget.
    fn fake_sys(serial: Option<&str>) -> PathBuf {
        let root = std::env::temp_dir().join(format!("fakesys_{}_{}", std::process::id(), serial.unwrap_or("none")));
        let _ = std::fs::remove_dir_all(&root);
        let usbdev = root.join("devices/platform/soc/xhci/usb1/1-1");
        let intf = usbdev.join("1-1:1.0");
        std::fs::create_dir_all(intf.join("net/enp0s20f0u1")).unwrap();
        std::fs::write(usbdev.join("idVendor"), "2edd\n").unwrap();
        std::fs::write(usbdev.join("idProduct"), "0001\n").unwrap();
        std::fs::write(usbdev.join("product"), "reMarkable 2\n").unwrap();
        if let Some(s) = serial {
            std::fs::write(usbdev.join("serial"), format!("{s}\n")).unwrap();
        }
        let net = root.join("class/net/enp0s20f0u1");
        std::fs::create_dir_all(&net).unwrap();
        std::os::unix::fs::symlink(&intf, net.join("device")).unwrap();
        std::fs::write(net.join("address"), "02:ab:cd:ef:01:02\n").unwrap();
        // a non-USB interface
        let eth = root.join("class/net/eth0");
        std::fs::create_dir_all(root.join("devices/pci0000:00/0000:00:1f.6")).unwrap();
        std::fs::create_dir_all(&eth).unwrap();
        std::os::unix::fs::symlink(root.join("devices/pci0000:00/0000:00:1f.6"), eth.join("device")).unwrap();
        root
    }

    #[test]
    fn the_usb_device_behind_an_interface_is_found_and_keyed_by_serial() {
        let sys = fake_sys(Some("RM110-313-00000"));
        let l = usb_link("enp0s20f0u1", &sys).expect("usb interface");
        assert_eq!((l.vendor.as_str(), l.product.as_str()), ("2edd", "0001"));
        assert_eq!(l.product_name.as_deref(), Some("reMarkable 2"));
        assert_eq!(l.key(), "2edd:0001:RM110-313-00000");
        assert_eq!(l.alias(), "gitmind-2edd-0001-rm110-313-00000", "lowercase: OpenSSH lowercases host names");
        assert!(usb_link("eth0", &sys).is_none(), "PCI ethernet is not USB");
        assert!(usb_link("nope", &sys).is_none());
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn two_tablets_on_the_same_address_get_different_aliases() {
        let a = fake_sys(Some("RM110-AAA"));
        let b = fake_sys(Some("RM110-BBB"));
        let (la, lb) = (usb_link("enp0s20f0u1", &a).unwrap(), usb_link("enp0s20f0u1", &b).unwrap());
        assert_ne!(la.alias(), lb.alias(), "an rM2 and a Paper Pro on 10.11.99.1 are different trust entries");
        let o = ssh_opts(&la);
        assert!(o.iter().any(|x| x == &format!("HostKeyAlias={}", la.alias())));
        assert!(o.iter().any(|x| x.starts_with("UserKnownHostsFile=") && x.ends_with("tablet_known_hosts")), "never the user's file");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn without_a_serial_the_mac_identifies_and_says_so() {
        let sys = fake_sys(None);
        let l = usb_link("enp0s20f0u1", &sys).unwrap();
        assert!(!l.has_serial());
        assert_eq!(l.key(), "2edd:0001:mac-02:ab:cd:ef:01:02");
        assert!(l.describe().contains("identified by MAC"));
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn scanned_keys_are_filed_under_the_device_alias() {
        let scanned = "10.11.99.1 ssh-ed25519 AAAAC3Nza\n10.11.99.1 ssh-rsa AAAAB3Nza";
        assert_eq!(alias_lines(scanned, "gitmind-x"), "gitmind-x ssh-ed25519 AAAAC3Nza\ngitmind-x ssh-rsa AAAAB3Nza");
    }

    #[test]
    fn untrust_removes_hashed_entries_too() {
        // what ssh itself writes when the system hashes known_hosts
        let dir = std::env::temp_dir().join(format!("khh_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("kh");
        let plain = "gitmind-2edd-0001-x ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIB8Qb7wQ1Qb7wQ1Qb7wQ1Qb7wQ1Qb7wQ1Qb7wQ1Qb7wQ\n";
        std::fs::write(&f, plain).unwrap();
        let hashed = std::process::Command::new("ssh-keygen").arg("-H").arg("-f").arg(&f).output().unwrap();
        if !hashed.status.success() { return; } // no ssh-keygen here
        assert!(std::fs::read_to_string(&f).unwrap().starts_with("|1|"), "entry is hashed now");
        assert!(known("gitmind-2edd-0001-x", &f), "lookup finds the hashed entry");
        let o = std::process::Command::new("ssh-keygen").arg("-R").arg("gitmind-2edd-0001-x").arg("-f").arg(&f).output().unwrap();
        assert!(o.status.success());
        assert!(!known("gitmind-2edd-0001-x", &f), "and removal removes it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn untrust_removes_only_that_device() {
        let dir = std::env::temp_dir().join(format!("kh_{}", std::process::id()));
        std::env::set_var("XDG_STATE_HOME", &dir);
        std::fs::create_dir_all(known_hosts_path().parent().unwrap()).unwrap();
        let sys = fake_sys(Some("RM110-KEEP"));
        let l = usb_link("enp0s20f0u1", &sys).unwrap();
        std::fs::write(known_hosts_path(), format!("{} ssh-ed25519 A\ngitmind-other ssh-ed25519 B\n", l.alias())).unwrap();
        untrust(&l).unwrap();
        assert_eq!(std::fs::read_to_string(known_hosts_path()).unwrap(), "gitmind-other ssh-ed25519 B\n");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn route_output_names_the_interface() {
        assert_eq!(parse_route_dev("10.11.99.1 dev enp0s20f0u1 src 10.11.99.2 uid 1000 \n    cache "), Some("enp0s20f0u1".into()));
        assert_eq!(parse_route_dev("unreachable"), None);
        assert!(route_iface("10.11.99.1; rm -rf /").is_none(), "only literal addresses reach the shell-free call");
    }

    #[test]
    fn the_cookie_is_created_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("cookie_{}", std::process::id()));
        let f = dir.join("device-id");
        let cmd = identity_cmd().replace(COOKIE, &f.to_string_lossy());
        let run = || String::from_utf8(std::process::Command::new("sh").arg("-c").arg(&cmd).output().unwrap().stdout).unwrap();
        let (a, b) = (parse_identity(&run()), parse_identity(&run()));
        assert_eq!(a.device_id.len(), 32, "{a:?}");
        assert_eq!(a.device_id, b.device_id, "stable across contacts");
        assert!(a.hostname.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
