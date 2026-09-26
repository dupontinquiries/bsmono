//! SurfaceLink against a real compositor: weston's headless backend, which
//! implements wp_presentation (but neither content-type nor commit timing).
//! A client maps a toplevel, commits a new shm buffer at 30 fps for a
//! second, and the feedback must describe weston's output.

use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vidkit_wayland::cadence::{Outcome, PresentLog};

use vidkit_wayland::surface::SurfaceLink;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default)]
struct App {
    configured: Option<u32>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
    fn event(_: &mut Self, b: &xdg_wm_base::XdgWmBase, e: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = e {
            b.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(s: &mut Self, _: &xdg_surface::XdgSurface, e: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = e {
            s.configured = Some(serial);
        }
    }
}
macro_rules! ignore {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore!(wl_compositor::WlCompositor, wl_surface::WlSurface, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer, xdg_toplevel::XdgToplevel);

struct Weston(std::process::Child, std::path::PathBuf);
impl Drop for Weston {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn weston() -> Option<(Weston, Connection)> {
    let dir = std::env::temp_dir().join(format!("vk-wl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
    let child = Command::new("weston")
        .args(["--backend=headless", "--socket=vk-test", "--idle-time=0"])
        .env("XDG_RUNTIME_DIR", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let w = Weston(child, dir.clone());
    let sock = dir.join("vk-test");
    let t0 = Instant::now();
    while !sock.exists() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let stream = std::os::unix::net::UnixStream::connect(&sock).ok()?;
    let conn = Connection::from_socket(stream).ok()?;
    Some((w, conn))
}

#[test]
fn feedback_describes_westons_output() {
    let Some((_weston, conn)) = weston() else {
        eprintln!("no weston; skipped");
        return;
    };
    let (globals, mut q) = registry_queue_init::<App>(&conn).unwrap();
    let qh = q.handle();
    let comp: wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = comp.create_surface(&qh, ());
    let xs = wm.get_xdg_surface(&surface, &qh, ());
    let top = xs.get_toplevel(&qh, ());
    top.set_title("vidkit-wayland test".into());
    surface.commit();
    let mut app = App::default();
    while app.configured.is_none() {
        q.blocking_dispatch(&mut app).unwrap();
    }
    xs.ack_configure(app.configured.unwrap());

    // Two 64x64 XRGB buffers, alternated: each commit is new content.
    let (w, h) = (64i32, 64i32);
    let size = (w * h * 4 * 2) as usize;
    let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk".as_ptr(), 0)) };
    assert_eq!(unsafe { libc::ftruncate(std::os::fd::AsRawFd::as_raw_fd(&fd), size as i64) }, 0);
    let pool = shm.create_pool(fd.as_fd(), size as i32, &qh, ());
    let bufs: Vec<_> = (0..2).map(|i| pool.create_buffer(i * w * h * 4, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ())).collect();

    let mut link = SurfaceLink::new(conn.clone(), surface.clone()).unwrap();
    let caps = link.caps();
    eprintln!("DIAG caps {caps:?}");
    assert!(caps.presentation && caps.clock_id.is_some(), "weston has wp_presentation");
    assert!(!caps.content_type && !caps.commit_timing, "weston 13 has neither; the gating must say so");

    let mut log = PresentLog::new(128);
    let t0 = Instant::now();
    let period = Duration::from_micros(33_333);
    for k in 0..36u32 {
        let due = t0 + period * k;
        if let Some(d) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        let target = link.clock_now_ns();
        surface.attach(Some(&bufs[(k % 2) as usize]), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        link.before_commit(target);
        surface.commit();
        conn.flush().unwrap();
        let _ = q.dispatch_pending(&mut app);
        for o in link.poll() {
            if k < 4 {
                eprintln!("DIAG raw {o:?}");
            }
            log.push(o);
        }
    }
    let t1 = Instant::now();
    while t1.elapsed() < Duration::from_millis(300) {
        std::thread::sleep(Duration::from_millis(10));
        for o in link.poll() {
            log.push(o);
        }
    }
    let summary = log.summary(Some(30.0));
    eprintln!("DIAG {} presented, {} discarded: {summary}; holds {:?}", log.presented, log.discarded, log.hold_histogram());
    assert!(log.presented >= 30, "feedback for most frames: {}", log.presented);
    let hz = 1e9 / log.refresh_ns().expect("weston reports a refresh") as f64;
    assert!((hz - 60.0).abs() < 1.0, "headless weston runs at 60 Hz, got {hz:.2}");
    // 30 fps on 60 Hz is all 2s at best; a one-core sandbox's sleeps are
    // late now and then, so some 3s (and 1s after them) are expected.
    let j = log.judder(30.0).unwrap();
    assert!(j < 0.5, "judder {j}");
    let (median, _) = log.lateness_ns().expect("targets were set");
    assert!(median >= 0 && median < 40_000_000, "presented after it was meant, within about a refresh or two: {median} ns");
    // Headless weston repaints off a timer: software timestamps, so no
    // verdict on VRR -- not a false "variable".
    let _: Option<Outcome> = None;
    if log.variable().is_some() {
        panic!("a verdict from a software clock: {:?}", log.variable());
    }
    let _ = top;
}
