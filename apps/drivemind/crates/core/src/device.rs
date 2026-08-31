// device — enumerate block devices and classify how they're attached.
//
// Source of truth is `lsblk --json -b -o …` (util-linux, present everywhere
// including postmarketOS). We ask for bytes (-b) so sizes are numbers, not
// "3.6T" strings.
//
// The classification that actually matters downstream is `Attach`:
//
//   * NativeNvme  — /dev/nvme*: full NVMe admin passthrough, smartctl just works.
//   * NativeSata  — SATA/AHCI: ATA SMART, works.
//   * UsbBridge   — the interesting one. An external "NVMe SSD" enclosure is
//     an NVMe drive behind a USB-to-NVMe bridge chip (JMicron JMS583,
//     ASMedia ASM236x, Realtek RTL9210, …). Linux sees a USB mass-storage
//     (UAS) disk named /dev/sdX, and plain `smartctl -a /dev/sdX` typically
//     reports NOTHING useful because SMART commands must be tunneled through
//     the bridge's vendor protocol. smart.rs owns the retry ladder
//     (`-d sat` → `-d sntjmicron` → `-d sntasmedia` → `-d sntrealtek`);
//     this module's job is to flag WHICH devices need it.
//   * Virtual     — virtio/loop/zram/md/dm: no SMART to speak of; skip
//     health probing instead of reporting scary false negatives.

use crate::{SysOutput, SysRunner};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Attach {
    NativeNvme,
    NativeSata,
    UsbBridge,
    Virtual,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockDevice {
    /// Kernel name ("sda", "nvme0n1").
    pub name: String,
    /// Device node ("/dev/sda").
    pub path: String,
    pub size_bytes: u64,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub vendor: Option<String>,
    /// lsblk TRAN: "nvme" | "sata" | "usb" | "virtio" | null …
    pub transport: Option<String>,
    /// Rotational (true = spinning disk).
    pub rotational: bool,
    /// Removable flag as the kernel sees it (unreliable for USB SSDs, but
    /// still worth surfacing).
    pub removable: bool,
    pub attach: Attach,
    /// Mountpoints of the disk and its partitions (for clone safety checks
    /// and for "is this the disk you think it is" UI).
    pub mountpoints: Vec<String>,
}

impl BlockDevice {
    /// Devices for which a SMART probe is meaningful.
    pub fn smart_capable(&self) -> bool {
        matches!(self.attach, Attach::NativeNvme | Attach::NativeSata | Attach::UsbBridge)
    }
}

// ---- lsblk JSON shape (only the fields we ask for) ----

#[derive(Deserialize)]
struct LsblkRoot {
    blockdevices: Vec<LsblkDev>,
}

#[derive(Deserialize)]
struct LsblkDev {
    name: String,
    path: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    // -b makes this numeric, but lsblk emits null for zram-without-size etc.
    size: Option<u64>,
    tran: Option<String>,
    model: Option<String>,
    serial: Option<String>,
    vendor: Option<String>,
    rota: Option<bool>,
    rm: Option<bool>,
    mountpoint: Option<String>,
    #[serde(default)]
    children: Vec<LsblkDev>,
}

pub const LSBLK_ARGS: &[&str] = &[
    "--json",
    "-b",
    "-o",
    "NAME,PATH,TYPE,SIZE,TRAN,MODEL,SERIAL,VENDOR,ROTA,RM,MOUNTPOINT",
];

/// Enumerate disks via the runner. Partitions are folded into their parent
/// (mountpoints collected); only TYPE=="disk" nodes become devices.
pub fn enumerate(sys: &dyn SysRunner) -> Result<Vec<BlockDevice>, String> {
    let out = sys.run("lsblk", LSBLK_ARGS)?;
    parse_lsblk(&out)
}

fn classify(tran: Option<&str>, name: &str) -> Attach {
    match tran {
        Some("nvme") => Attach::NativeNvme,
        Some("sata") | Some("ata") | Some("sas") => Attach::NativeSata,
        Some("usb") => Attach::UsbBridge,
        Some("virtio") => Attach::Virtual,
        _ => {
            if name.starts_with("nvme") {
                Attach::NativeNvme
            } else if name.starts_with("loop")
                || name.starts_with("zram")
                || name.starts_with("dm-")
                || name.starts_with("md")
                || name.starts_with("vd")
            {
                Attach::Virtual
            } else {
                Attach::Unknown
            }
        }
    }
}

fn parse_lsblk(out: &SysOutput) -> Result<Vec<BlockDevice>, String> {
    let root: LsblkRoot =
        serde_json::from_str(&out.stdout).map_err(|e| format!("lsblk json: {e}"))?;
    let mut devs = Vec::new();
    for d in root.blockdevices {
        if d.kind.as_deref() != Some("disk") {
            continue;
        }
        let mut mountpoints = Vec::new();
        if let Some(m) = &d.mountpoint {
            mountpoints.push(m.clone());
        }
        collect_mounts(&d.children, &mut mountpoints);
        let attach = classify(d.tran.as_deref(), &d.name);
        devs.push(BlockDevice {
            path: d.path.clone().unwrap_or_else(|| format!("/dev/{}", d.name)),
            name: d.name,
            size_bytes: d.size.unwrap_or(0),
            model: none_if_blank(d.model),
            serial: none_if_blank(d.serial),
            vendor: none_if_blank(d.vendor),
            transport: d.tran,
            rotational: d.rota.unwrap_or(false),
            removable: d.rm.unwrap_or(false),
            attach,
            mountpoints,
        });
    }
    Ok(devs)
}

fn collect_mounts(children: &[LsblkDev], into: &mut Vec<String>) {
    for c in children {
        if let Some(m) = &c.mountpoint {
            into.push(m.clone());
        }
        collect_mounts(&c.children, into);
    }
}

fn none_if_blank(s: Option<String>) -> Option<String> {
    s.and_then(|v| {
        let t = v.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FixtureRunner;

    const FIX: &str = r#"{"blockdevices":[
      {"name":"nvme0n1","path":"/dev/nvme0n1","type":"disk","size":1024209543168,
       "tran":"nvme","model":"WD_BLACK SN850X","serial":"A1","vendor":null,
       "rota":false,"rm":false,"mountpoint":null,
       "children":[{"name":"nvme0n1p1","path":"/dev/nvme0n1p1","type":"part",
         "size":1024,"tran":null,"model":null,"serial":null,"vendor":null,
         "rota":false,"rm":false,"mountpoint":"/","children":[]}]},
      {"name":"sda","path":"/dev/sda","type":"disk","size":2000398934016,
       "tran":"usb","model":"SSD 980 PRO 2TB","serial":"B2","vendor":"Samsung ",
       "rota":false,"rm":true,"mountpoint":null,"children":[]},
      {"name":"zram0","path":"/dev/zram0","type":"disk","size":null,
       "tran":null,"model":null,"serial":null,"vendor":null,
       "rota":false,"rm":false,"mountpoint":null,"children":[]}
    ]}"#;

    fn cmdline() -> String {
        std::iter::once("lsblk")
            .chain(LSBLK_ARGS.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn parses_and_classifies() {
        let sys = FixtureRunner::new().on(&cmdline(), 0, FIX);
        let devs = enumerate(&sys).unwrap();
        assert_eq!(devs.len(), 3);
        assert_eq!(devs[0].attach, Attach::NativeNvme);
        assert_eq!(devs[0].mountpoints, vec!["/"]);
        // The external "NVMe SSD" shows up as USB — the bridge case.
        assert_eq!(devs[1].attach, Attach::UsbBridge);
        assert!(devs[1].smart_capable());
        assert_eq!(devs[1].vendor.as_deref(), Some("Samsung"));
        assert_eq!(devs[2].attach, Attach::Virtual);
        assert!(!devs[2].smart_capable());
        assert_eq!(devs[2].size_bytes, 0);
    }
}
