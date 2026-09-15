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

    #[test]
    fn sata_ata_and_sas_transports_all_classify_as_native_sata() {
        // Completely untested before this: the only transports the
        // existing fixture exercised were nvme, usb, and null (virtual).
        // NativeSata is reachable through three distinct lsblk TRAN
        // strings -- each needs its own check, since a match arm typo'd
        // to handle only one of the three would not be caught by testing
        // just one.
        for tran in ["sata", "ata", "sas"] {
            let fix = format!(
                r#"{{"blockdevices":[{{"name":"sda","path":"/dev/sda","type":"disk","size":100,
                   "tran":"{tran}","model":null,"serial":null,"vendor":null,
                   "rota":true,"rm":false,"mountpoint":null,"children":[]}}]}}"#
            );
            let sys = FixtureRunner::new().on(&cmdline(), 0, &fix);
            let devs = enumerate(&sys).unwrap();
            assert_eq!(devs.len(), 1);
            assert_eq!(devs[0].attach, Attach::NativeSata, "tran={tran}");
            assert!(devs[0].smart_capable(), "tran={tran}");
        }
    }

    #[test]
    fn a_genuinely_unrecognized_transport_and_name_falls_to_unknown_not_a_guess() {
        // The fallback classifier only recognizes specific NAME prefixes
        // (nvme, loop, zram, dm-, md, vd) when TRAN itself is absent or
        // unrecognized. A device matching none of those must land on
        // Unknown rather than being silently mis-sorted into one of the
        // real categories -- Unknown existing at all is the module
        // admitting what it does not know, which is safer than a wrong
        // guess feeding into a SMART probe or a clone-safety check.
        let fix = r#"{"blockdevices":[{"name":"xvda","path":"/dev/xvda","type":"disk","size":100,
            "tran":null,"model":null,"serial":null,"vendor":null,
            "rota":false,"rm":false,"mountpoint":null,"children":[]}]}"#;
        let sys = FixtureRunner::new().on(&cmdline(), 0, fix);
        let devs = enumerate(&sys).unwrap();
        assert_eq!(devs[0].attach, Attach::Unknown);
        assert!(!devs[0].smart_capable(), "Unknown must not be probed as if it were a real transport");
    }

    #[test]
    fn a_non_disk_top_level_entry_is_excluded() {
        // The existing fixture's only non-"disk" entries are NESTED
        // partitions under a disk. A top-level sibling of a different
        // type (a "rom" entry for a CD/DVD drive is the real-world case)
        // must also be excluded by the same `kind != "disk"` check --
        // untested at the top level specifically.
        let fix = r#"{"blockdevices":[
            {"name":"sda","path":"/dev/sda","type":"disk","size":100,
             "tran":"sata","model":null,"serial":null,"vendor":null,
             "rota":true,"rm":false,"mountpoint":null,"children":[]},
            {"name":"sr0","path":"/dev/sr0","type":"rom","size":0,
             "tran":"ata","model":null,"serial":null,"vendor":null,
             "rota":false,"rm":true,"mountpoint":null,"children":[]}
        ]}"#;
        let sys = FixtureRunner::new().on(&cmdline(), 0, fix);
        let devs = enumerate(&sys).unwrap();
        assert_eq!(devs.len(), 1, "the rom entry must not appear as a device: {devs:?}");
        assert_eq!(devs[0].name, "sda");
    }

    #[test]
    fn mountpoints_are_collected_through_multiple_levels_of_children() {
        // The existing fixture only ever had ONE level of children
        // (disk -> partition, mounted). A real LVM stack is disk ->
        // partition -> logical volume, mounted two levels down --
        // collect_mounts is recursive specifically for this, and the
        // existing test could pass even if the recursive call were
        // accidentally removed, since one level deep does not need it.
        let fix = r#"{"blockdevices":[{"name":"sda","path":"/dev/sda","type":"disk","size":100,
            "tran":"sata","model":null,"serial":null,"vendor":null,
            "rota":true,"rm":false,"mountpoint":null,
            "children":[{"name":"sda1","path":"/dev/sda1","type":"part","size":50,
                "tran":null,"model":null,"serial":null,"vendor":null,
                "rota":true,"rm":false,"mountpoint":null,
                "children":[{"name":"vg-lv","path":"/dev/mapper/vg-lv","type":"lvm","size":40,
                    "tran":null,"model":null,"serial":null,"vendor":null,
                    "rota":true,"rm":false,"mountpoint":"/home","children":[]}]}]}]}"#;
        let sys = FixtureRunner::new().on(&cmdline(), 0, fix);
        let devs = enumerate(&sys).unwrap();
        assert_eq!(devs[0].mountpoints, vec!["/home"],
            "a mountpoint two levels down through an LVM stack must still be found");
    }

    #[test]
    fn a_zero_length_model_string_is_treated_the_same_as_absent() {
        // The existing test only ever checked whitespace-trimming on
        // VENDOR ("Samsung " -> "Samsung"). A genuinely EMPTY string
        // (lsblk's real behavior for a field it has nothing to report,
        // some kernels/versions use "" rather than null) needs its own
        // check on MODEL specifically, since none_if_blank is called
        // three separate times (model, serial, vendor) and a mistake in
        // any one of those three call sites would not be caught by
        // testing only one of them.
        let fix = r#"{"blockdevices":[{"name":"sda","path":"/dev/sda","type":"disk","size":100,
            "tran":"sata","model":"","serial":"","vendor":null,
            "rota":true,"rm":false,"mountpoint":null,"children":[]}]}"#;
        let sys = FixtureRunner::new().on(&cmdline(), 0, fix);
        let devs = enumerate(&sys).unwrap();
        assert_eq!(devs[0].model, None, "an empty string must become None, not Some(\"\")");
        assert_eq!(devs[0].serial, None);
    }

    #[test]
    fn a_failing_lsblk_command_is_a_real_error_not_an_empty_device_list() {
        // If lsblk itself fails to run (missing binary, permission
        // denied), enumerate() must propagate that as an Err a caller
        // can act on and show -- not silently return an empty Vec, which
        // would look identical to "this machine genuinely has no disks"
        // and could feed a clone-planning UI a device list that is wrong
        // for a completely different reason than it appears to be.
        struct AlwaysFails;
        impl SysRunner for AlwaysFails {
            fn run(&self, _: &str, _: &[&str]) -> Result<SysOutput, String> {
                Err("lsblk: command not found".into())
            }
        }
        let err = enumerate(&AlwaysFails).expect_err("a failed command must not look like zero disks");
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn malformed_json_from_lsblk_is_a_real_error_not_a_panic_or_empty_list() {
        // A different failure shape than the command failing outright:
        // lsblk RUNS (exit 0) but its output is not the JSON this parser
        // expects -- a lsblk version mismatch, a truncated pipe, a
        // locale wrapping numbers in a way that breaks the parser. Must
        // surface as an Err, not panic (this runs inside a GUI event
        // loop, where a panic takes the whole app down over one
        // unexpected line of output) and not silently succeed with an
        // empty list indistinguishable from "no disks."
        let sys = FixtureRunner::new().on(&cmdline(), 0, "not valid json at all");
        let err = enumerate(&sys).expect_err("malformed JSON must not parse into an empty Vec");
        assert!(err.contains("lsblk json"), "{err}");
    }
}
