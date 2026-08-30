// cloneop — whole-device cloning as a PLAN first, an action reluctantly.
//
// Cloning is one command and a catastrophe if the arguments are swapped.
// So the shape here is: build a `ClonePlan` that (a) runs every safety
// check against the enumerated device list and (b) contains the exact
// command line — then frontends SHOW the plan, and only `execute(force)`
// runs it. A plan that fails any check refuses to execute regardless of
// force; force only skips the "are you sure" layer, never the physics.
//
// Checks (all must pass):
//   * src and dst are distinct devices that exist in the enumeration
//   * dst (or any of its partitions) is not mounted — cloning under a live
//     filesystem corrupts both the clone and possibly the mount
//   * dst capacity ≥ src capacity
//   * src is not empty-sized (0-byte source means enumeration lied)
//
// Tool choice: ddrescue when the SOURCE is suspected unhealthy (it retries
// and maps bad sectors instead of dying at the first one, and its mapfile
// makes the copy resumable); plain dd for a healthy source (faster path,
// present everywhere). The caller passes that suspicion in — typically
// derived from smart::probe's worst() — so policy stays at the edge.

use crate::device::BlockDevice;
use crate::{human_bytes, Event, Sink, SysRunner};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloneTool {
    Dd,
    Ddrescue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClonePlan {
    pub src: String,
    pub dst: String,
    pub tool: CloneTool,
    pub src_bytes: u64,
    pub dst_bytes: u64,
    /// The exact command that would run.
    pub command: String,
    /// Failed checks. Empty = executable.
    pub blockers: Vec<String>,
    /// Non-blocking things a human should read before confirming.
    pub warnings: Vec<String>,
}

impl ClonePlan {
    pub fn ok(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// Build (and check) a plan. `src_unhealthy` selects ddrescue.
pub fn plan(
    devices: &[BlockDevice],
    src: &str,
    dst: &str,
    src_unhealthy: bool,
) -> ClonePlan {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    let find = |p: &str| devices.iter().find(|d| d.path == p || d.name == p);
    let s = find(src);
    let d = find(dst);

    if src == dst {
        blockers.push("source and destination are the same device".into());
    }
    if s.is_none() {
        blockers.push(format!("source {src} not found among enumerated disks"));
    }
    if d.is_none() {
        blockers.push(format!("destination {dst} not found among enumerated disks"));
    }

    let (mut src_bytes, mut dst_bytes) = (0, 0);
    if let Some(s) = s {
        src_bytes = s.size_bytes;
        if src_bytes == 0 {
            blockers.push(format!("source {src} reports 0 bytes — refusing to trust it"));
        }
        if !s.mountpoints.is_empty() {
            warnings.push(format!(
                "source is mounted at {} — clone will be crash-consistent at best; unmount for a faithful copy",
                s.mountpoints.join(", ")
            ));
        }
    }
    if let Some(d) = d {
        dst_bytes = d.size_bytes;
        if !d.mountpoints.is_empty() {
            blockers.push(format!(
                "destination is mounted at {} — unmount everything on it first",
                d.mountpoints.join(", ")
            ));
        }
        if let Some(s) = s {
            if d.size_bytes < s.size_bytes {
                blockers.push(format!(
                    "destination too small: {} < {}",
                    human_bytes(d.size_bytes),
                    human_bytes(s.size_bytes)
                ));
            }
        }
        warnings.push(format!(
            "EVERYTHING on {dst} ({}, {}) will be overwritten",
            d.model.as_deref().unwrap_or("unknown model"),
            human_bytes(d.size_bytes)
        ));
    }

    let tool = if src_unhealthy { CloneTool::Ddrescue } else { CloneTool::Dd };
    let command = match tool {
        CloneTool::Dd => {
            format!("dd if={src} of={dst} bs=4M conv=fsync oflag=direct status=progress")
        }
        CloneTool::Ddrescue => {
            // The mapfile is the whole point: resumable, and a persistent
            // record of which sectors were unreadable.
            format!("ddrescue -f -n {src} {dst} /var/tmp/drivemind-clone.map")
        }
    };
    if tool == CloneTool::Ddrescue {
        warnings.push(
            "source flagged unhealthy → ddrescue (resumable, maps bad sectors); install gddrescue if missing"
                .into(),
        );
    }

    ClonePlan { src: src.into(), dst: dst.into(), tool, src_bytes, dst_bytes, command, blockers, warnings }
}

/// Execute a plan. Refuses without force, refuses with blockers always.
pub fn execute(sys: &dyn SysRunner, plan: &ClonePlan, force: bool, sink: Sink) -> Result<(), String> {
    if !plan.ok() {
        return Err(format!("plan has blockers: {}", plan.blockers.join("; ")));
    }
    if !force {
        return Err("refusing to clone without force=true — show the plan to the user first".into());
    }
    sink(Event::Exec(plan.command.clone()));
    let mut parts = plan.command.split_whitespace();
    let prog = parts.next().ok_or("empty command")?;
    let args: Vec<&str> = parts.collect();
    let out = sys.run(prog, &args)?;
    if out.code != 0 {
        return Err(format!("{prog} exited {}: {}", out.code, out.stderr.trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Attach, BlockDevice};
    use crate::{null_sink, FixtureRunner};

    fn dev(path: &str, size: u64, mounts: &[&str]) -> BlockDevice {
        BlockDevice {
            name: path.trim_start_matches("/dev/").to_string(),
            path: path.to_string(),
            size_bytes: size,
            model: Some("TestDisk".into()),
            serial: None,
            vendor: None,
            transport: None,
            rotational: false,
            removable: false,
            attach: Attach::Unknown,
            mountpoints: mounts.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn blockers_catch_the_classic_mistakes() {
        let devs = vec![
            dev("/dev/sda", 1000, &["/"]),
            dev("/dev/sdb", 500, &[]),
            dev("/dev/sdc", 2000, &["/mnt/backup"]),
        ];
        // Too small.
        assert!(!plan(&devs, "/dev/sda", "/dev/sdb", false).ok());
        // Mounted destination.
        let p = plan(&devs, "/dev/sda", "/dev/sdc", false);
        assert!(p.blockers.iter().any(|b| b.contains("mounted")));
        // Same device.
        assert!(!plan(&devs, "/dev/sda", "/dev/sda", false).ok());
        // Unknown destination.
        assert!(!plan(&devs, "/dev/sda", "/dev/sdz", false).ok());
    }

    #[test]
    fn healthy_plan_executes_only_with_force() {
        let devs = vec![dev("/dev/sda", 1000, &[]), dev("/dev/sdb", 2000, &[])];
        let p = plan(&devs, "/dev/sda", "/dev/sdb", false);
        assert!(p.ok(), "{:?}", p.blockers);
        assert_eq!(p.tool, CloneTool::Dd);
        assert!(p.command.starts_with("dd if=/dev/sda of=/dev/sdb"));

        let sys = FixtureRunner::new().on(&p.command, 0, "");
        let mut sink = null_sink();
        assert!(execute(&sys, &p, false, &mut sink).is_err());
        assert!(execute(&sys, &p, true, &mut sink).is_ok());
    }

    #[test]
    fn unhealthy_source_selects_ddrescue_and_mounted_source_only_warns() {
        let devs = vec![dev("/dev/sda", 1000, &["/data"]), dev("/dev/sdb", 2000, &[])];
        let p = plan(&devs, "/dev/sda", "/dev/sdb", true);
        assert!(p.ok());
        assert_eq!(p.tool, CloneTool::Ddrescue);
        assert!(p.command.contains("ddrescue"));
        assert!(p.warnings.iter().any(|w| w.contains("crash-consistent")));
    }
}
