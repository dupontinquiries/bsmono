//! appinfo_io -- reads `/proc` and desktop entries for `tideshell_core::appinfo`.
//!
//! The model there is pure; this is the part that touches the system, kept small and split into
//! parsers (tested on strings in the formats the kernel really writes) and one poller that is
//! cheap by construction: it lists `/proc` at most once per `INTERVAL_MS`, and only while someone
//! asks about a window (the carousel is up), so an idle phone pays nothing.

use std::collections::HashMap;
use std::path::Path;
use tideshell_core::appinfo::{self, AppKey, Info, Proc};

/// How often the process table is re-read while it is wanted.
pub const INTERVAL_MS: u64 = 2000;

/// (utime + stime, starttime) in clock ticks, from `/proc/<pid>/stat`. The command name is in
/// parentheses and may itself contain spaces and parentheses, so fields are counted from the
/// LAST `)`.
pub fn parse_stat(text: &str) -> Option<(u64, u64)> {
    let rest = &text[text.rfind(')')? + 1..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After the ')' the fields restart at 3 (state), so utime is field 14 = index 11.
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    let start: u64 = f.get(19)?.parse().ok()?;
    Some((utime + stime, start))
}

/// Resident set in kB from `/proc/<pid>/statm` (second field, in pages).
pub fn parse_statm_rss_kb(text: &str, page_kb: u64) -> Option<u64> {
    Some(text.split_whitespace().nth(1)?.parse::<u64>().ok()? * page_kb)
}

/// Seconds since boot from `/proc/uptime`.
pub fn parse_uptime(text: &str) -> Option<f32> {
    text.split_whitespace().next()?.parse().ok()
}

/// The binary a desktop entry runs: the first word of `Exec=` that is not `env`, a `VAR=value`
/// or a flag, as a basename. `Exec=env GDK_BACKEND=wayland /usr/bin/kgx --foo %U` -> `kgx`.
pub fn exec_from_desktop(text: &str) -> Option<String> {
    let line = text.lines().find_map(|l| l.trim().strip_prefix("Exec="))?;
    let word = words(line).into_iter().find(|w| w != "env" && !w.contains('=') && !w.starts_with('-'))?;
    Path::new(&word).file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Split on whitespace, but not inside double quotes (desktop-entry `Exec` quoting).
fn words(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Every process the user can read, as the model wants it.
pub fn list_procs(page_kb: u64) -> Vec<Proc> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return out };
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let base = format!("/proc/{pid}");
        let (Some(stat), Some(statm)) = (read(format!("{base}/stat")), read(format!("{base}/statm"))) else { continue };
        let (Some((cpu_ticks, start_ticks)), Some(rss_kb)) = (parse_stat(&stat), parse_statm_rss_kb(&statm, page_kb)) else { continue };
        let comm = read(format!("{base}/comm")).unwrap_or_default().trim().to_string();
        let cmdline = read(format!("{base}/cmdline")).unwrap_or_default().replace('\0', " ").trim().to_string();
        let exe = std::fs::read_link(format!("{base}/exe"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .or_else(|| cmdline.split_whitespace().next().and_then(|a| Path::new(a).file_name()).map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        out.push(Proc { pid, comm, exe, cmdline, cpu_ticks, start_ticks, rss_kb });
    }
    out
}

/// The desktop entry's binary for an app id, looked up once per app and remembered.
fn exec_hint(app_id: &str, cache: &mut HashMap<String, Option<String>>) -> Option<String> {
    cache
        .entry(app_id.to_string())
        .or_insert_with(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            [format!("{home}/.local/share/applications"), "/usr/local/share/applications".into(), "/usr/share/applications".into()]
                .iter()
                .find_map(|d| read(format!("{d}/{app_id}.desktop")))
                .and_then(|t| exec_from_desktop(&t))
        })
        .clone()
}

/// Polls the process table for the windows it is asked about.
#[derive(Default)]
pub struct Poller {
    last_ms: Option<u64>,
    before: HashMap<u32, Proc>,
    infos: HashMap<u64, Info>,
    hints: HashMap<String, Option<String>>,
}

impl Poller {
    /// The latest facts for one window, without touching the table.
    pub fn info(&self, id: u64) -> Option<Info> {
        self.infos.get(&id).copied()
    }

    /// Latest facts per window id. Re-reads `/proc` when `INTERVAL_MS` has passed since the last
    /// read; between reads it returns the last answer for free. `wanted` is (window id, app id).
    pub fn poll(&mut self, now_ms: u64, wanted: &[(u64, String)]) -> &HashMap<u64, Info> {
        if wanted.is_empty() {
            self.last_ms = None;
            self.before.clear();
            self.infos.clear();
            return &self.infos;
        }
        let due = self.last_ms.is_none_or(|t| now_ms.saturating_sub(t) >= INTERVAL_MS);
        if !due {
            return &self.infos;
        }
        let dt = self.last_ms.map_or(0.0, |t| now_ms.saturating_sub(t) as f32 / 1000.0);
        let hz = 100; // USER_HZ: what /proc reports ticks in, on every Linux userspace
        let page_kb = 4;
        let procs = list_procs(page_kb);
        let boot = read("/proc/uptime").and_then(|t| parse_uptime(&t)).unwrap_or(0.0);
        let mut infos = HashMap::new();
        for (id, app_id) in wanted {
            let key = AppKey { app_id: app_id.clone(), exec_hint: exec_hint(app_id, &mut self.hints) };
            let pids = appinfo::match_pids(&key, &procs);
            let after: Vec<Proc> = procs.iter().filter(|p| pids.contains(&p.pid)).cloned().collect();
            let before: Vec<Proc> = after.iter().filter_map(|p| self.before.get(&p.pid).cloned()).collect();
            if !after.is_empty() {
                infos.insert(*id, appinfo::sample(&before, &after, dt, hz, boot));
            }
        }
        self.before = procs.into_iter().map(|p| (p.pid, p)).collect();
        self.last_ms = Some(now_ms);
        self.infos = infos;
        &self.infos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_is_read_from_the_last_paren_so_odd_command_names_cannot_shift_the_fields() {
        // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime stime cutime cstime priority nice threads itreal starttime ...
        let plain = "4321 (beaver-notes) S 1 4321 4321 0 -1 4194304 100 0 0 0 250 50 0 0 20 0 12 0 98765 1000 2000";
        assert_eq!(parse_stat(plain), Some((300, 98765)));
        let nasty = "77 (a (b) c) S 1 77 77 0 -1 0 0 0 0 0 10 5 0 0 20 0 1 0 555 1 2";
        assert_eq!(parse_stat(nasty), Some((15, 555)), "a name with spaces and parens");
        assert_eq!(parse_stat("garbage"), None);
        assert_eq!(parse_stat("1 (x) S 1 1"), None, "truncated");
    }

    #[test]
    fn rss_and_uptime_parse() {
        assert_eq!(parse_statm_rss_kb("5000 1234 300 10 0 900 0", 4), Some(4936));
        assert_eq!(parse_statm_rss_kb("5000", 4), None);
        assert_eq!(parse_uptime("1962.55 14562.12\n"), Some(1962.55));
        assert_eq!(parse_uptime(""), None);
    }

    #[test]
    fn the_exec_binary_skips_env_assignments_flags_and_paths() {
        assert_eq!(exec_from_desktop("[Desktop Entry]\nName=x\nExec=kgx %U\n").as_deref(), Some("kgx"));
        assert_eq!(exec_from_desktop("Exec=env GDK_BACKEND=wayland /usr/bin/kgx --foo %U").as_deref(), Some("kgx"));
        assert_eq!(exec_from_desktop("Exec=\"/opt/Beaver Notes/beaver-notes\" --no-sandbox").as_deref(), Some("beaver-notes"), "quoted path");
        assert_eq!(exec_from_desktop("Name=no exec here"), None);
        assert_eq!(exec_from_desktop("Exec=env A=b"), None, "nothing but assignments");
    }

    #[test]
    fn the_poller_reads_this_process_and_is_free_between_intervals() {
        // The test binary is a process with a name we can look up: ask about ourselves.
        let me = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        let mut p = Poller::default();
        let wanted = vec![(1u64, me.clone())];
        let first = p.poll(10_000, &wanted).get(&1).copied();
        let first = first.unwrap_or_else(|| panic!("the poller found no process for {me}"));
        assert!(first.rss_mb > 0.5, "a running test binary has resident memory: {first:?}");
        assert!(first.procs >= 1);
        // inside the interval: the same answer, no re-read (the table's timestamp does not move)
        let again = p.poll(10_000 + INTERVAL_MS - 1, &wanted).get(&1).copied();
        assert_eq!(again, Some(first));
        // nobody asking clears the state, so a closed carousel keeps nothing
        assert!(p.poll(20_000, &[]).is_empty());
        assert!(p.last_ms.is_none() && p.before.is_empty());
        // the second real sample has an interval, so CPU is computed (>= 0, finite)
        p.poll(30_000, &wanted);
        let second = p.poll(30_000 + INTERVAL_MS, &wanted).get(&1).copied().unwrap();
        assert!(second.cpu_pct.is_finite() && second.cpu_pct >= 0.0);
    }
}
