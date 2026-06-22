// Sets BUILD_TIMESTAMP env var at compile time so the running binary can
// report when it was built. This is helpful for verifying that a fresh
// build is actually what's running — we've hit cases where stale binaries
// reported old behavior and a visible build stamp made it easy to diagnose.
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // ISO-style date in UTC. Cheap to compute, doesn't need chrono.
    let date = unix_to_iso(secs);
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", date);
    // Re-run if the version changes (Cargo handles this automatically for
    // changed source files, but printing this makes intent clear).
    println!("cargo:rerun-if-changed=Cargo.toml");
}

fn unix_to_iso(t: u64) -> String {
    // Days since epoch
    let days = (t / 86400) as i64;
    let mut sec_of_day = (t % 86400) as u32;
    let h = sec_of_day / 3600; sec_of_day %= 3600;
    let m = sec_of_day / 60;
    let s = sec_of_day % 60;
    // Convert days to YYYY-MM-DD using Howard Hinnant's algorithm
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m_out = if mp < 10 { mp + 3 } else { mp - 9 };
    let y_out = if m_out <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}Z", y_out, m_out, d, h, m, s)
}
