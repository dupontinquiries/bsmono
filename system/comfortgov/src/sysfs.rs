//! Discovery, sampling and writes under a root (`/` in production, a
//! temporary tree in tests). Nothing here decides anything.

use crate::policy::{Cluster, Decision, Window};
use std::path::{Path, PathBuf};

pub struct Sys {
    pub root: PathBuf,
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_u64(p: &Path) -> Option<u64> {
    read(p)?.parse().ok()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub clusters: Vec<Cluster>,
    pub cluster_dirs: Vec<PathBuf>,
    pub zones: Vec<(String, PathBuf, Option<f32>)>,
    /// The chosen skin sensor: (name, path, offset °C subtracted).
    pub skin: Option<(String, PathBuf, f32)>,
    pub battery: Option<PathBuf>,
    pub has_mglru: bool,
    pub has_psi: bool,
}

/// Which thermal zone stands for the hand. Real skin sensors first; a
/// battery pack sits against the hand too; a CPU zone only with an offset
/// (the die runs well above the case).
pub fn pick_skin(zones: &[(String, PathBuf, Option<f32>)]) -> Option<(String, PathBuf, f32)> {
    let lower = |s: &str| s.to_ascii_lowercase();
    let find = |pats: &[&str]| zones.iter().find(|(n, _, t)| t.is_some() && pats.iter().any(|p| lower(n).contains(p)));
    if let Some((n, p, _)) = find(&["skin", "case", "quiet", "back", "shell", "surface"]) {
        return Some((n.clone(), p.clone(), 0.0));
    }
    if let Some((n, p, _)) = find(&["batt", "bms"]) {
        return Some((n.clone(), p.clone(), 0.0));
    }
    // Hottest CPU zone, minus the die-to-case offset.
    zones
        .iter()
        .filter(|(n, _, t)| t.is_some() && lower(n).contains("cpu"))
        .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|(n, p, _)| (n.clone(), p.clone(), CPU_TO_SKIN_OFFSET_C))
}

/// A CPU die reads well above the case; a conservative estimate.
pub const CPU_TO_SKIN_OFFSET_C: f32 = 12.0;

impl Sys {
    pub fn new(root: impl Into<PathBuf>) -> Sys {
        Sys { root: root.into() }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }

    pub fn probe(&self, floors: &[(String, u32)]) -> Probe {
        let mut clusters = Vec::new();
        let mut cluster_dirs = Vec::new();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(self.p("sys/devices/system/cpu/cpufreq"))
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("policy"))).collect())
            .unwrap_or_default();
        dirs.sort();
        for dir in dirs {
            let name = dir.file_name().unwrap().to_string_lossy().into_owned();
            let mut freqs: Vec<u32> = read(&dir.join("scaling_available_frequencies"))
                .map(|s| s.split_whitespace().filter_map(|f| f.parse().ok()).collect())
                .unwrap_or_default();
            if freqs.is_empty() {
                // Drivers without the list: the two ends are still a window.
                if let (Some(lo), Some(hi)) = (read_u64(&dir.join("cpuinfo_min_freq")), read_u64(&dir.join("cpuinfo_max_freq"))) {
                    freqs = vec![lo as u32, hi as u32];
                }
            }
            freqs.sort_unstable();
            freqs.dedup();
            if freqs.is_empty() {
                continue;
            }
            let floor_khz = floors.iter().find(|(n, _)| *n == name).map(|(_, f)| *f);
            clusters.push(Cluster { name, freqs, floor_khz });
            cluster_dirs.push(dir);
        }
        let mut zones = Vec::new();
        if let Ok(d) = std::fs::read_dir(self.p("sys/class/thermal")) {
            let mut zs: Vec<PathBuf> = d.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone"))).collect();
            zs.sort();
            for z in zs {
                let name = read(&z.join("type")).unwrap_or_default();
                let temp = read_u64(&z.join("temp")).map(|m| m as f32 / 1000.0);
                zones.push((name, z.join("temp"), temp));
            }
        }
        let skin = pick_skin(&zones);
        let battery = std::fs::read_dir(self.p("sys/class/power_supply"))
            .ok()
            .and_then(|d| d.flatten().map(|e| e.path()).find(|p| read(&p.join("type")).as_deref() == Some("Battery")));
        Probe {
            clusters,
            cluster_dirs,
            zones,
            skin,
            battery,
            has_mglru: self.p("sys/kernel/mm/lru_gen/min_ttl_ms").exists(),
            has_psi: self.p("proc/pressure/memory").exists(),
        }
    }

    pub fn skin_c(&self, skin: &Option<(String, PathBuf, f32)>) -> Option<f32> {
        let (_, p, off) = skin.as_ref()?;
        read_u64(p).map(|m| m as f32 / 1000.0 - off)
    }

    pub fn battery_c(&self, dir: &Option<PathBuf>) -> Option<f32> {
        read_u64(&dir.as_ref()?.join("temp")).map(|t| t as f32 / 10.0)
    }

    /// (on_battery, percent).
    pub fn battery(&self, dir: &Option<PathBuf>) -> (bool, u32) {
        let Some(d) = dir else { return (false, 100) };
        let status = read(&d.join("status")).unwrap_or_default();
        let on_battery = matches!(status.as_str(), "Discharging" | "Not charging");
        (on_battery, read_u64(&d.join("capacity")).unwrap_or(100) as u32)
    }

    /// `/proc/stat`'s first line as (busy, total) jiffies.
    pub fn cpu_jiffies(&self) -> Option<(u64, u64)> {
        let s = read(&self.p("proc/stat"))?;
        let line = s.lines().next()?;
        let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
        if v.len() < 4 {
            return None;
        }
        let idle = v[3] + v.get(4).copied().unwrap_or(0);
        let total: u64 = v.iter().sum();
        Some((total - idle, total))
    }

    pub fn psi_mem_some(&self) -> f32 {
        read(&self.p("proc/pressure/memory"))
            .and_then(|s| s.lines().find(|l| l.starts_with("some")).map(str::to_string))
            .and_then(|l| l.split_whitespace().find_map(|kv| kv.strip_prefix("avg10=").and_then(|v| v.parse::<f32>().ok())))
            .map(|pct| pct / 100.0)
            .unwrap_or(0.0)
    }

    pub fn mem_avail_frac(&self) -> f32 {
        let Some(s) = read(&self.p("proc/meminfo")) else { return 1.0 };
        let get = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok());
        match (get("MemTotal:"), get("MemAvailable:")) {
            (Some(t), Some(a)) if t > 0.0 => (a / t) as f32,
            _ => 1.0,
        }
    }

    /// The order that never trips cpufreq's min<=max check: raise the max
    /// before the min when the window moves up, lower the min before the
    /// max when it moves down.
    pub fn write_order(new: Window) -> [(&'static str, u32); 2] {
        [("scaling_max_freq", new.max_khz), ("scaling_min_freq", new.min_khz)]
    }

    pub fn write_window(&self, dir: &Path, cur: Option<Window>, new: Window) -> std::io::Result<()> {
        let moving_up = cur.is_none_or(|c| new.max_khz >= c.max_khz);
        let mut order = Self::write_order(new);
        if !moving_up {
            order.reverse();
        }
        for (f, v) in order {
            std::fs::write(dir.join(f), v.to_string())?;
        }
        Ok(())
    }

    /// App scopes under `dir` (tideshell names them `app-tideshell-<id>-<pid>.scope`).
    pub fn app_scopes(dir: &Path) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("app-tideshell-"))).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Which scope to reclaim from this tick: round-robin over the scopes
    /// that are NOT the foreground app's (the id sits in the scope's name,
    /// with the same character substitution tideshell applies).
    pub fn pick_reclaim_target(scopes: &[PathBuf], foreground_app: &str, cursor: usize) -> Option<usize> {
        let fg: String = foreground_app.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '_' }).collect();
        let candidates: Vec<usize> = scopes
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                fg.is_empty() || !n.starts_with(&format!("app-tideshell-{fg}-"))
            })
            .map(|(i, _)| i)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some(candidates[cursor % candidates.len()])
    }

    pub fn apply(&self, probe: &Probe, cur: &[Option<Window>], d: &Decision, bg_cgroup: Option<&Path>) -> Vec<String> {
        let mut log = Vec::new();
        for (i, dir) in probe.cluster_dirs.iter().enumerate() {
            let w = d.windows[i];
            if cur.get(i).copied().flatten() == Some(w) {
                continue;
            }
            match self.write_window(dir, cur.get(i).copied().flatten(), w) {
                Ok(()) => log.push(format!("{}: {}-{} MHz", probe.clusters[i].name, w.min_khz / 1000, w.max_khz / 1000)),
                Err(e) => log.push(format!("{}: write failed: {e}", probe.clusters[i].name)),
            }
        }
        let sw = self.p("proc/sys/vm/swappiness");
        if read_u64(&sw) != Some(d.swappiness as u64) {
            match std::fs::write(&sw, d.swappiness.to_string()) {
                Ok(()) => log.push(format!("swappiness {}", d.swappiness)),
                Err(e) => log.push(format!("swappiness: {e}")),
            }
        }
        if probe.has_mglru {
            let ttl = self.p("sys/kernel/mm/lru_gen/min_ttl_ms");
            if read_u64(&ttl) != Some(d.min_ttl_ms as u64) {
                match std::fs::write(&ttl, d.min_ttl_ms.to_string()) {
                    Ok(()) => log.push(format!("lru_gen min_ttl_ms {}", d.min_ttl_ms)),
                    Err(e) => log.push(format!("min_ttl_ms: {e}")),
                }
            }
        }
        if d.reclaim_bytes > 0 {
            if let Some(cg) = bg_cgroup {
                match std::fs::write(cg.join("memory.reclaim"), d.reclaim_bytes.to_string()) {
                    Ok(()) => log.push(format!("reclaimed {} MiB from {}", d.reclaim_bytes >> 20, cg.display())),
                    Err(e) => log.push(format!("reclaim: {e}")),
                }
            }
        }
        log
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    fn fake(tag: &str) -> Sys {
        let root = std::env::temp_dir().join(format!("comfortgov-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (pol, freqs) in [("policy0", "300000 576000 1804800"), ("policy4", "691200 2400000")] {
            let d = root.join("sys/devices/system/cpu/cpufreq").join(pol);
            put(&d.join("scaling_available_frequencies"), freqs);
            put(&d.join("scaling_min_freq"), "300000");
            put(&d.join("scaling_max_freq"), "1804800");
        }
        let d = root.join("sys/devices/system/cpu/cpufreq/policy7");
        put(&d.join("cpuinfo_min_freq"), "800000");
        put(&d.join("cpuinfo_max_freq"), "2700000");
        for (i, (t, temp)) in [("cpu-0-0", "55000"), ("cpu-1-0", "61000"), ("battery", "31000"), ("gpu", "48000")].iter().enumerate() {
            let z = root.join(format!("sys/class/thermal/thermal_zone{i}"));
            put(&z.join("type"), t);
            put(&z.join("temp"), temp);
        }
        let b = root.join("sys/class/power_supply/battery");
        put(&b.join("type"), "Battery");
        put(&b.join("status"), "Discharging");
        put(&b.join("capacity"), "63");
        put(&root.join("proc/stat"), "cpu  100 0 50 850 0 0 0 0 0 0\n");
        put(&root.join("proc/pressure/memory"), "some avg10=2.50 avg60=1.00 avg300=0.50 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n");
        put(&root.join("proc/meminfo"), "MemTotal:  8000000 kB\nMemFree:   1000000 kB\nMemAvailable: 2000000 kB\n");
        put(&root.join("proc/sys/vm/swappiness"), "60");
        put(&root.join("sys/kernel/mm/lru_gen/min_ttl_ms"), "0");
        Sys::new(root)
    }

    #[test]
    fn discovers_clusters_zones_battery_and_picks_the_skin_proxy() {
        let sys = fake("probe");
        let p = sys.probe(&[("policy4".into(), 700_000)]);
        assert_eq!(p.clusters.len(), 3);
        assert_eq!(p.clusters[0].freqs, vec![300_000, 576_000, 1_804_800]);
        assert_eq!(p.clusters[1].floor_khz, Some(700_000));
        assert_eq!(p.clusters[2].freqs, vec![800_000, 2_700_000], "min/max fallback without the list");
        // No skin zone: the battery pack is the proxy, no offset.
        let (name, _, off) = p.skin.clone().unwrap();
        assert_eq!((name.as_str(), off), ("battery", 0.0));
        assert_eq!(sys.skin_c(&p.skin), Some(31.0));
        assert_eq!(sys.battery(&p.battery), (true, 63));
        assert!(p.has_mglru && p.has_psi);
        let _ = std::fs::remove_dir_all(&sys.root);
    }

    #[test]
    fn skin_pick_prefers_real_skin_sensors_and_offsets_cpu_fallbacks() {
        let z = |n: &str, t: f32| (n.to_string(), PathBuf::from(n), Some(t));
        let picked = pick_skin(&[z("cpu-0-0", 60.0), z("skin-therm", 35.0), z("battery", 31.0)]).unwrap();
        assert_eq!(picked.0, "skin-therm");
        let picked = pick_skin(&[z("cpu-0-0", 60.0), z("cpu-1-1", 64.0), z("gpu", 50.0)]).unwrap();
        assert_eq!((picked.0.as_str(), picked.2), ("cpu-1-1", CPU_TO_SKIN_OFFSET_C), "hottest CPU zone, with the die-to-case offset");
        assert!(pick_skin(&[("gpu".into(), PathBuf::new(), Some(50.0))]).is_none());
    }

    #[test]
    fn samples_util_psi_and_memory() {
        let sys = fake("sample");
        let (busy, total) = sys.cpu_jiffies().unwrap();
        assert_eq!((busy, total), (150, 1000));
        assert!((sys.psi_mem_some() - 0.025).abs() < 1e-6);
        assert!((sys.mem_avail_frac() - 0.25).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&sys.root);
    }

    #[test]
    fn write_window_moving_down_writes_min_before_max() {
        // Observable through a failing max: moving down, min must already be
        // on disk when max fails; the wrong order would fail before min.
        let sys = fake("order");
        let dir = sys.root.join("sys/devices/system/cpu/cpufreq/policy0");
        std::fs::write(dir.join("scaling_min_freq"), "576000").unwrap();
        std::fs::remove_file(dir.join("scaling_max_freq")).unwrap();
        std::fs::create_dir(dir.join("scaling_max_freq")).unwrap(); // unwritable
        let cur = Some(Window { min_khz: 576_000, max_khz: 1_804_800 });
        let r = sys.write_window(&dir, cur, Window { min_khz: 300_000, max_khz: 576_000 });
        assert!(r.is_err());
        assert_eq!(read(&dir.join("scaling_min_freq")).as_deref(), Some("300000"), "min was written first");
        let _ = std::fs::remove_dir_all(&sys.root);
    }

    #[test]
    fn reclaim_rotates_over_background_scopes_and_spares_the_front_app() {
        let root = std::env::temp_dir().join(format!("comfortgov-scopes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for n in ["app-tideshell-org.gnome.Console-100.scope", "app-tideshell-org.mozilla.firefox-200.scope", "app-tideshell-org.mozilla.firefox-300.scope", "session.scope"] {
            std::fs::create_dir_all(root.join(n)).unwrap();
        }
        let scopes = Sys::app_scopes(&root);
        assert_eq!(scopes.len(), 3, "only tideshell app scopes");
        // Firefox in front (both its windows' scopes spared): only Console is a target.
        for c in 0..4 {
            let i = Sys::pick_reclaim_target(&scopes, "org.mozilla.firefox", c).unwrap();
            assert!(scopes[i].ends_with("app-tideshell-org.gnome.Console-100.scope"));
        }
        // Console in front: the two firefox scopes alternate.
        let a = Sys::pick_reclaim_target(&scopes, "org.gnome.Console", 0).unwrap();
        let b = Sys::pick_reclaim_target(&scopes, "org.gnome.Console", 1).unwrap();
        assert_ne!(a, b);
        // Nothing in front: everything rotates.
        assert!(Sys::pick_reclaim_target(&scopes, "", 5).is_some());
        // Only the front app running: nothing to reclaim.
        assert_eq!(Sys::pick_reclaim_target(&scopes[..1], "org.gnome.Console", 0), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_writes_windows_in_a_safe_order_and_only_changes() {
        let sys = fake("apply");
        let p = sys.probe(&[]);
        let d = Decision {
            state: crate::policy::State::Interactive,
            windows: vec![Window { min_khz: 576_000, max_khz: 1_804_800 }, Window { min_khz: 691_200, max_khz: 2_400_000 }, Window { min_khz: 800_000, max_khz: 2_700_000 }],
            ceiling: 1.0,
            swappiness: 40,
            min_ttl_ms: 1000,
            reclaim_bytes: 0,
            why: String::new(),
        };
        let log = sys.apply(&p, &[None, None, None], &d, None);
        assert!(log.iter().any(|l| l.contains("swappiness 40")) && log.iter().any(|l| l.contains("min_ttl_ms 1000")), "{log:?}");
        let d0 = &p.cluster_dirs[0];
        assert_eq!(read(&d0.join("scaling_min_freq")).as_deref(), Some("576000"));
        assert_eq!(read(&d0.join("scaling_max_freq")).as_deref(), Some("1804800"));
        // Second apply with the same windows: nothing written for clusters.
        let cur: Vec<Option<Window>> = d.windows.iter().copied().map(Some).collect();
        let log2 = sys.apply(&p, &cur, &d, None);
        assert!(log2.iter().all(|l| !l.contains("MHz")), "{log2:?}");
        // Moving down: min is written before max.
        let down = Window { min_khz: 300_000, max_khz: 576_000 };
        let mut order = Sys::write_order(down);
        order.reverse();
        assert_eq!(order[0].0, "scaling_min_freq");
        assert_eq!(Sys::write_order(down)[0].0, "scaling_max_freq", "moving up: max first");
        let _ = std::fs::remove_dir_all(&sys.root);
    }
}
