//! appinfo -- live facts about an app, for the carousel peeks (and any future "card" for a window).
//!
//! The compositor tells the shell a window's title, app id and size; it does NOT say which process
//! owns it. So this module does two pure jobs, and does no I/O of its own:
//!
//! 1. [`match_pids`]: which processes belong to an app id (the frontend lists `/proc`, this picks).
//! 2. [`sample`] and [`Info`]: from two `/proc` readings of those processes, the CPU share over the
//!    interval, resident memory and age, plus the formatting a 36-point-wide strip needs.
//!
//! Matching is a ladder, best evidence first, and the FIRST rung that finds anything wins (so a
//! loose rung never adds strangers to a precise one): exact executable name, the desktop entry's
//! `Exec=` binary, the kernel's 15-character `comm`, then the app id appearing in a command line.
//! Apps that run as several processes (a browser, an Electron app) are summed.

use std::collections::HashMap;

/// One running process, as the frontend read it from `/proc/<pid>`.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: u32,
    /// `/proc/<pid>/comm`: cut to 15 characters by the kernel.
    pub comm: String,
    /// The basename of `/proc/<pid>/exe` (or of argv[0] when exe is unreadable).
    pub exe: String,
    pub cmdline: String,
    /// utime + stime, in clock ticks.
    pub cpu_ticks: u64,
    /// Start time, in clock ticks after boot (field 22 of `/proc/<pid>/stat`).
    pub start_ticks: u64,
    pub rss_kb: u64,
}

/// A window's identity as far as processes go.
#[derive(Clone, Debug, Default)]
pub struct AppKey {
    pub app_id: String,
    /// The binary named by the app's desktop entry (`Exec=`), when the frontend found one.
    pub exec_hint: Option<String>,
}

/// The last path segment of a reverse-DNS app id, lowercased: `org.gnome.Console` -> `console`.
pub fn tail(app_id: &str) -> String {
    app_id.rsplit('.').next().unwrap_or(app_id).trim().to_lowercase()
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// The processes that belong to `key`, from `procs`. Empty when nothing is convincing.
pub fn match_pids(key: &AppKey, procs: &[Proc]) -> Vec<u32> {
    let full = norm(&key.app_id);
    let last = norm(&tail(&key.app_id));
    if full.is_empty() {
        return Vec::new();
    }
    let rungs: [Box<dyn Fn(&Proc) -> bool>; 4] = [
        // 1. the executable IS the app (vidkit-player, beaver-notes, hatch-egui for "hatch-egui")
        Box::new(|p: &Proc| {
            let e = norm(&p.exe);
            !e.is_empty() && (e == full || e == last)
        }),
        // 2. the desktop entry says which binary the app is
        Box::new(|p: &Proc| key.exec_hint.as_ref().is_some_and(|h| !norm(h).is_empty() && norm(&p.exe) == norm(h))),
        // 3. comm, which the kernel cuts at 15 characters: compare against the cut name
        Box::new(|p: &Proc| {
            let c = norm(&p.comm);
            c.len() >= 4 && (full.starts_with(&c) || last.starts_with(&c))
        }),
        // 4. the id or its tail named on the command line (an interpreter running the app)
        Box::new(|p: &Proc| last.len() >= 4 && norm(&p.cmdline).contains(&last)),
    ];
    for rung in rungs.iter() {
        let pids: Vec<u32> = procs.iter().filter(|p| rung(p)).map(|p| p.pid).collect();
        if !pids.is_empty() {
            return pids;
        }
    }
    Vec::new()
}

/// What the strip shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Info {
    /// Share of ONE core over the sample interval, summed over the app's processes (100 = a core).
    pub cpu_pct: f32,
    pub rss_mb: f32,
    /// The age of the app's oldest process.
    pub uptime_s: u64,
    pub procs: u32,
}

/// Two readings of the same app's processes `dt_s` apart. A process missing from `before` (it
/// started in between) contributes memory and age but no CPU for this interval.
pub fn sample(before: &[Proc], after: &[Proc], dt_s: f32, hz: u64, boot_uptime_s: f32) -> Info {
    let prev: HashMap<u32, u64> = before.iter().map(|p| (p.pid, p.cpu_ticks)).collect();
    let mut ticks = 0u64;
    let mut oldest = 0u64;
    let mut rss = 0u64;
    for p in after {
        if let Some(b) = prev.get(&p.pid) {
            ticks += p.cpu_ticks.saturating_sub(*b);
        }
        rss += p.rss_kb;
        let age = (boot_uptime_s - p.start_ticks as f32 / hz.max(1) as f32).max(0.0) as u64;
        oldest = oldest.max(age);
    }
    let cpu = if dt_s > 0.0 { ticks as f32 / hz.max(1) as f32 / dt_s * 100.0 } else { 0.0 };
    Info { cpu_pct: cpu, rss_mb: rss as f32 / 1024.0, uptime_s: oldest, procs: after.len() as u32 }
}

/// `7%`, `130%`: whole numbers; under 1% is `<1`, so an idle app does not read as `0`.
pub fn fmt_cpu(pct: f32) -> String {
    if pct < 0.5 {
        "<1%".into()
    } else {
        format!("{:.0}%", pct)
    }
}

/// `48M`, `1.2G`: four characters wide at most.
pub fn fmt_mem(mb: f32) -> String {
    if mb >= 1000.0 {
        format!("{:.1}G", mb / 1024.0)
    } else {
        format!("{:.0}M", mb)
    }
}

/// `45s`, `12m`, `3h`, `2d`: the largest unit that is at least 1.
pub fn fmt_age(s: u64) -> String {
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, comm: &str, exe: &str, cmd: &str, ticks: u64, start: u64, rss: u64) -> Proc {
        Proc { pid, comm: comm.into(), exe: exe.into(), cmdline: cmd.into(), cpu_ticks: ticks, start_ticks: start, rss_kb: rss }
    }

    fn key(id: &str, hint: Option<&str>) -> AppKey {
        AppKey { app_id: id.into(), exec_hint: hint.map(str::to_string) }
    }

    #[test]
    fn the_tail_of_a_reverse_dns_id_is_its_last_segment() {
        assert_eq!(tail("org.gnome.Console"), "console");
        assert_eq!(tail("vidkit-player"), "vidkit-player");
        assert_eq!(tail("com.system76.CosmicSettings"), "cosmicsettings");
    }

    #[test]
    fn an_executable_named_like_the_app_wins_and_sums_its_processes() {
        let procs = vec![
            p(10, "beaver-notes", "beaver-notes", "beaver-notes", 1, 1, 100),
            p(11, "beaver-notes", "beaver-notes", "beaver-notes --type=gpu", 1, 1, 100),
            p(12, "beaver-notes", "beaver-notes", "beaver-notes --type=renderer", 1, 1, 100),
            p(20, "other", "other", "other", 1, 1, 1),
        ];
        assert_eq!(match_pids(&key("beaver-notes", None), &procs), vec![10, 11, 12], "all three of one app");
        assert_eq!(match_pids(&key("org.beaver.beaver-notes", None), &procs), vec![10, 11, 12], "by the tail of the id");
    }

    #[test]
    fn the_desktop_entry_names_the_binary_when_the_app_id_does_not() {
        let procs = vec![p(30, "kgx", "kgx", "kgx", 1, 1, 1), p(31, "console-helper", "console-helper", "x", 1, 1, 1)];
        assert!(match_pids(&key("org.gnome.Console", None), &procs).is_empty(), "no hint: nothing convincing");
        assert_eq!(match_pids(&key("org.gnome.Console", Some("kgx")), &procs), vec![30], "the hint finds it");
    }

    #[test]
    fn a_cut_comm_matches_and_a_precise_rung_is_never_diluted_by_a_loose_one() {
        // `comm` is cut to 15 chars: "xdg-desktop-por" for xdg-desktop-portal-cosmic
        let procs = vec![p(40, "xdg-desktop-por", "", "", 1, 1, 1)];
        assert_eq!(match_pids(&key("xdg-desktop-portal-cosmic", None), &procs), vec![40]);
        // exe match exists, so the cmdline stranger is not added
        let procs = vec![
            p(50, "hatch-egui", "hatch-egui", "hatch-egui", 1, 1, 1),
            p(51, "python3", "python3", "python3 /opt/hatch-egui-tools/run.py", 1, 1, 1),
        ];
        assert_eq!(match_pids(&key("hatch-egui", None), &procs), vec![50]);
        // and with no better evidence the command line is used
        let procs = vec![p(60, "python3", "python3", "python3 /home/u/dissent/app.py", 1, 1, 1)];
        assert_eq!(match_pids(&key("org.example.Dissent", None), &procs), vec![60]);
        assert!(match_pids(&key("", None), &procs).is_empty(), "no id, no match");
        assert!(match_pids(&key("a.b", None), &[p(1, "zzz", "zzz", "run ab now", 1, 1, 1)]).is_empty(), "too short to trust on a command line");
        assert_eq!(match_pids(&key("a.b", None), &[p(1, "ab", "ab", "ab", 1, 1, 1)]), vec![1], "but an exact executable name is always enough");
    }

    #[test]
    fn cpu_is_the_tick_delta_over_the_interval_summed_across_processes() {
        let before = vec![p(1, "a", "a", "a", 100, 0, 0), p(2, "a", "a", "a", 50, 0, 0)];
        let after = vec![p(1, "a", "a", "a", 150, 0, 2048), p(2, "a", "a", "a", 60, 0, 2048)];
        let i = sample(&before, &after, 2.0, 100, 1000.0);
        // (50 + 10) ticks = 0.6 s of CPU over 2 s = 30% of a core
        assert!((i.cpu_pct - 30.0).abs() < 0.01, "{}", i.cpu_pct);
        assert!((i.rss_mb - 4.0).abs() < 0.01);
        assert_eq!(i.procs, 2);
        assert_eq!(sample(&before, &after, 0.0, 100, 1000.0).cpu_pct, 0.0, "a zero interval is not a division");
    }

    #[test]
    fn a_process_that_started_between_samples_adds_memory_and_age_but_no_cpu() {
        let before = vec![p(1, "a", "a", "a", 100, 0, 0)];
        let after = vec![p(1, "a", "a", "a", 100, 0, 1024), p(9, "a", "a", "a", 5000, 99_000, 1024)];
        let i = sample(&before, &after, 1.0, 100, 1000.0);
        assert_eq!(i.cpu_pct, 0.0, "the newcomer's lifetime ticks are not this second's work");
        assert!((i.rss_mb - 2.0).abs() < 0.01);
        assert_eq!(i.uptime_s, 1000, "age is the oldest process's: started at tick 0, boot uptime 1000 s");
    }

    #[test]
    fn the_strip_formats_stay_narrow() {
        assert_eq!(fmt_cpu(0.2), "<1%");
        assert_eq!(fmt_cpu(7.4), "7%");
        assert_eq!(fmt_cpu(130.0), "130%");
        assert_eq!(fmt_mem(48.2), "48M");
        assert_eq!(fmt_mem(1536.0), "1.5G");
        assert_eq!([fmt_age(45), fmt_age(720), fmt_age(7200), fmt_age(200_000)], ["45s", "12m", "2h", "2d"]);
        for s in [fmt_cpu(99.0), fmt_mem(999.0), fmt_age(86_399)] {
            assert!(s.len() <= 4, "{s} must fit a 36 pt strip");
        }
    }
}
