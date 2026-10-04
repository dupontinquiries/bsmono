//! penlog -- what does this digitizer actually report, and how fast?
//!
//! `penlog /dev/input/event2 20` reads that device for 20 s (read-only, shares the device with whatever else reads it) and
//! prints: the axis ranges the driver declares; then, for what you did in those seconds, the report rate while a
//! tool was in range, the gaps between reports, which tools were seen (pen / eraser / finger), the hover distance and
//! pressure ranges, and every tool enter/leave with a time. No tablet has evtest, and the numbers decide questions that
//! guessing cannot: is the pen sampled fast enough for smooth ink, and does anything register when a non-standard
//! stylus comes near?
use std::io::Read;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

extern "C" {
    fn ioctl(fd: i32, req: u64, ...) -> i32;
    fn poll(fds: *mut PollFd, n: u64, timeout_ms: i32) -> i32;
}

#[repr(C)]
struct PollFd { fd: i32, events: i16, revents: i16 }

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const SYN_REPORT: u16 = 0;
const ABS: [(u16, &str); 7] = [(0, "X"), (1, "Y"), (0x18, "PRESSURE"), (0x19, "DISTANCE"), (0x1a, "TILT_X"), (0x1b, "TILT_Y"), (0x2f, "MT_SLOT")];
const KEYS: [(u16, &str); 6] = [(0x140, "TOOL_PEN"), (0x141, "TOOL_RUBBER"), (0x14a, "TOUCH"), (0x14b, "STYLUS"), (0x14c, "STYLUS2"), (0x145, "TOOL_FINGER")];

fn eviocgabs(abs: u16) -> u64 {
    (2u64 << 30) | (24u64 << 16) | ((b'E' as u64) << 8) | (0x40 + abs as u64)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).map(String::as_str).unwrap_or("/dev/input/event2");
    let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(15);
    let mut f = std::fs::File::open(path).unwrap_or_else(|e| { eprintln!("{path}: {e}"); std::process::exit(1) });
    println!("device {path}, {secs} s");
    for (code, name) in ABS {
        let mut info = [0i32; 6];
        let r = unsafe { ioctl(f.as_raw_fd(), eviocgabs(code), info.as_mut_ptr()) };
        if r == 0 { println!("  ABS_{name:<9} min {:>6} max {:>6} resolution {:>4}/mm fuzz {}", info[1], info[2], info[5], info[3]); }
    }
    println!("recording... use the pen / a stylus / a finger now");
    let t0 = Instant::now();
    let mut buf = [0u8; 24 * 64];
    let (mut reports, mut in_range_reports) = (0u64, 0u64);
    let mut last_report: Option<Instant> = None;
    let mut gaps: Vec<f64> = Vec::new();
    let mut tool_down = [false; KEYS.len()];
    // multitouch: contacts down per slot (ABS_MT_TRACKING_ID >= 0), for a touch device that reports no BTN_TOOL_*
    let (mut slot, mut slots_down, mut mt_seen) = (0usize, [false; 16], false);
    let mut seen = [false; KEYS.len()];
    let (mut dist, mut pres) = ((i32::MAX, i32::MIN), (i32::MAX, i32::MIN));
    let mut log: Vec<String> = Vec::new();
    // non-blocking-ish read with a deadline: set O_NONBLOCK and poll
    unsafe { let fl = libc_fcntl(f.as_raw_fd(), 3, 0); libc_fcntl(f.as_raw_fd(), 4, fl | 0o4000); }
    while t0.elapsed() < Duration::from_secs(secs) {
        let n = match f.read(&mut buf) { Ok(n) => n, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // block until the device has something (or 200 ms, to notice the deadline): no busy wait on a battery
                let mut p = PollFd { fd: f.as_raw_fd(), events: 1, revents: 0 };
                unsafe { poll(&mut p, 1, 200) };
                continue
            } Err(_) => break };
        let now = Instant::now();
        for ev in buf[..n].chunks_exact(24) {
            let (ty, code, val) = (u16::from_ne_bytes([ev[16], ev[17]]), u16::from_ne_bytes([ev[18], ev[19]]), i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]));
            match ty {
                EV_KEY => if let Some(i) = KEYS.iter().position(|(c, _)| *c == code) {
                    seen[i] = true;
                    if val != 2 && (val == 1) != tool_down[i] { tool_down[i] = val == 1; log.push(format!("{:>7.3}s {} {}", t0.elapsed().as_secs_f32(), KEYS[i].1, if val == 1 { "ENTER/DOWN" } else { "LEAVE/UP" })); }
                },
                EV_ABS if code == 0x2f => { slot = (val.max(0) as usize).min(15); }
                EV_ABS if code == 0x39 => { mt_seen = true; slots_down[slot] = val >= 0; }
                EV_ABS if code == 0x19 => { dist = (dist.0.min(val), dist.1.max(val)); }
                EV_ABS if code == 0x18 => { pres = (pres.0.min(val), pres.1.max(val)); }
                EV_SYN if code == SYN_REPORT => {
                    reports += 1;
                    if tool_down[0] || tool_down[1] || tool_down[5] || slots_down.iter().any(|d| *d) {
                        in_range_reports += 1;
                        if let Some(l) = last_report { gaps.push(now.duration_since(l).as_secs_f64() * 1000.0); }
                    }
                    last_report = Some(now);
                }
                _ => {}
            }
        }
    }
    println!("\nreports: {reports} total, {in_range_reports} with a tool in range");
    if gaps.len() > 5 {
        gaps.retain(|g| *g < 200.0); // drop the pauses between strokes
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
        let p = |q: f64| gaps[((gaps.len() - 1) as f64 * q) as usize];
        println!("gap between reports (ms): median {:.2}, mean {:.2}, p95 {:.2}, max {:.2}  => about {:.0} reports/s while drawing", p(0.5), mean, p(0.95), gaps[gaps.len() - 1], 1000.0 / mean);
    } else {
        println!("not enough in-range reports to measure a rate");
    }
    if mt_seen { println!("multitouch contacts were reported (ABS_MT_TRACKING_ID)"); }
    let names: Vec<&str> = KEYS.iter().zip(seen).filter(|(_, s)| *s).map(|((_, n), _)| *n).collect();
    println!("tools/buttons seen: {}", if names.is_empty() { "none".into() } else { names.join(", ") });
    if dist.0 != i32::MAX { println!("hover distance seen: {}..{}", dist.0, dist.1); }
    if pres.0 != i32::MAX { println!("pressure seen: {}..{}", pres.0, pres.1); }
    for l in log.iter().take(40) { println!("  {l}"); }
    if log.len() > 40 { println!("  ... {} more", log.len() - 40); }
}

extern "C" {
    #[link_name = "fcntl"]
    fn fcntl_raw(fd: i32, cmd: i32, ...) -> i32;
}
unsafe fn libc_fcntl(fd: i32, cmd: i32, arg: i32) -> i32 { fcntl_raw(fd, cmd, arg) }
