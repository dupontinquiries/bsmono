//! Watch the compositor's toplevel list and optionally send one request.
//!
//!   cargo run --features wayland_io --example toplevel_probe -- [SECS] [activate|close|max|unmax APP_ID]
//!
//! Prints every change as it is read (so a window opened AFTER the probe
//! starts must show up — that is the check for `pump`), the manager's
//! capabilities, and the result of the request once the target appears.

use std::time::{Duration, Instant};
use systemcompkit::toplevel::ToplevelRequest;
use systemcompkit::wayland_io::ToplevelSession;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let secs: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(5);
    let req = match (args.get(1).map(String::as_str), args.get(2)) {
        (Some("activate"), Some(a)) => Some((ToplevelRequest::Activate, a.clone())),
        (Some("close"), Some(a)) => Some((ToplevelRequest::Close, a.clone())),
        (Some("max"), Some(a)) => Some((ToplevelRequest::SetMaximized(true), a.clone())),
        (Some("unmax"), Some(a)) => Some((ToplevelRequest::SetMaximized(false), a.clone())),
        _ => None,
    };
    let mut s = match ToplevelSession::connect() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("connect: {e:?}");
            std::process::exit(1);
        }
    };
    let mut last = String::new();
    let mut sent = false;
    let mut caps_shown = false;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        let list = s.poll().list().to_vec();
        let now: Vec<String> = list
            .iter()
            .map(|t| {
                let g = t.geometry.map(|(x, y, w, h)| format!(" {w}x{h}+{x}+{y}")).unwrap_or_default();
                format!("{}:{}:{}{}{}{g}", t.id, t.app_id, t.title, if t.state.activated { " [active]" } else { "" }, if t.state.maximized { " [max]" } else { "" })
            })
            .collect();
        let snapshot = now.join(" | ");
        if snapshot != last {
            println!("{:>6}ms {} window(s): {}", t0.elapsed().as_millis(), list.len(), snapshot);
            last = snapshot;
        }
        if !caps_shown && s.capabilities().received {
            caps_shown = true;
            println!("caps: {:?}", s.capabilities());
        }
        if let (false, Some((r, app))) = (sent, &req) {
            if let Some(t) = list.iter().find(|t| &t.app_id == app) {
                if s.capabilities().received {
                    println!("request {r:?} -> {}: {:?}", t.id, s.request(t.id, *r));
                    sent = true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
