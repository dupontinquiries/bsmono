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

fn weston_gl() -> Option<(Weston, Connection)> {
    weston_gl_at("vk-wlgl", &[])
}

fn weston_gl_at(tag: &str, extra: &[&str]) -> Option<(Weston, Connection)> {
    let dir = std::env::temp_dir().join(format!("{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok()?;
    let child = Command::new("weston")
        .args(["--backend=headless", "--renderer=gl", "--socket=vk-gl", "--idle-time=0"])
        .args(extra)
        .env("XDG_RUNTIME_DIR", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let w = Weston(child, dir.clone());
    let sock = dir.join("vk-gl");
    let t0 = Instant::now();
    while !sock.exists() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let conn = Connection::from_socket(std::os::unix::net::UnixStream::connect(&sock).ok()?).ok()?;
    Some((w, conn))
}

/// The video plane on weston's GL renderer (which takes NV12 dma-bufs;
/// llvmpipe maps our memfds): a pool of three decoder buffers cycled at
/// 30 fps. Buffers are imported once each, every later frame is shown,
/// the compositor presents the subsurface's commits, and frames are held
/// only until released.
#[test]
fn nv12_buffers_reach_the_compositor_through_the_plane() {
    use vidkit_core::frame::{DmaBufImage, DmaBufPlane, Frame, FrameData, PlaneLayout};
    use vidkit_wayland::plane::VideoPlane;
    let Some((_weston, conn)) = weston_gl() else {
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
    let _top = xs.get_toplevel(&qh, ());
    surface.commit();
    let mut app = App::default();
    while app.configured.is_none() {
        q.blocking_dispatch(&mut app).unwrap();
    }
    xs.ack_configure(app.configured.unwrap());
    let (pw, ph) = (320i32, 200i32);
    let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk-parent".as_ptr(), 0)) };
    assert_eq!(unsafe { libc::ftruncate(std::os::fd::AsRawFd::as_raw_fd(&fd), (pw * ph * 4) as i64) }, 0);
    let pool = shm.create_pool(fd.as_fd(), pw * ph * 4, &qh, ());
    let pbuf = pool.create_buffer(0, pw, ph, pw * 4, wl_shm::Format::Xrgb8888, &qh, ());
    surface.attach(Some(&pbuf), 0, 0);
    surface.commit();
    conn.flush().unwrap();

    let mut link = SurfaceLink::new(conn.clone(), surface.clone()).unwrap();
    let mut plane = VideoPlane::new(&conn, &surface, false, false).unwrap();
    assert!(plane.supports(vidkit_core::dmabuf::DRM_FORMAT_NV12, 0), "weston GL takes linear NV12");
    surface.commit(); // applies the subsurface's place/position
    conn.flush().unwrap();

    // The decoder's pool: three NV12 buffers, 160x96, padded stride.
    let (w, h, stride) = (160u32, 96u32, 192u32);
    let pool_fds: Vec<OwnedFd> = (0..3)
        .map(|i| {
            let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk-nv12".as_ptr(), 0)) };
            let mut buf = vec![0u8; (stride * h * 3 / 2) as usize];
            for (k, b) in buf.iter_mut().enumerate() {
                *b = if k < (stride * h) as usize { 40 + 60 * i } else { 128 };
            }
            std::io::Write::write_all(&mut std::fs::File::from(fd.try_clone().unwrap()), &buf).unwrap();
            fd
        })
        .collect();
    let frame = |i: usize| Frame {
        data: FrameData::DmaBuf(DmaBufImage { planes: vec![DmaBufPlane { fd: pool_fds[i].try_clone().unwrap(), offset: 0, stride }], modifier: 0, fourcc: vidkit_core::dmabuf::DRM_FORMAT_NV12, owner: None }),
        width: w,
        height: h,
        layout: PlaneLayout::Nv12,
        color: Default::default(),
        pts: Duration::ZERO,
        duration: None,
        recycle: None,
        grain: None,
    };
    let (mut ok, mut pending, mut refused, mut max_held) = (0, 0, 0, 0);
    let mut log = PresentLog::new(128);
    let t0 = Instant::now();
    for k in 0..45usize {
        if let Some(d) = (t0 + Duration::from_micros(33_333) * k as u32).checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        link.feedback_on(plane.surface(), link.clock_now_ns());
        match plane.show(frame(k % 3), (20, 30, 240, 144)) {
            Ok(()) => ok += 1,
            Err((_, why)) if why.contains("pending") => pending += 1,
            Err((_, why)) => {
                refused += 1;
                eprintln!("DIAG refused: {why}");
            }
        }
        max_held = max_held.max(plane.in_flight());
        let _ = q.dispatch_pending(&mut app);
        for o in link.poll() {
            log.push(o);
        }
        plane.poll();
    }
    let t1 = Instant::now();
    while t1.elapsed() < Duration::from_millis(300) {
        std::thread::sleep(Duration::from_millis(10));
        plane.poll();
        for o in link.poll() {
            log.push(o);
        }
    }
    eprintln!("DIAG plane: {ok} shown, {pending} while importing, {refused} refused, max held {max_held}, after drain {}; feedback {} presented: {}", plane.in_flight(), log.presented, log.summary(Some(30.0)));
    assert_eq!(refused, 0);
    assert!(pending <= 6, "each of the 3 buffers imports once (a frame or two each): {pending}");
    assert!(ok >= 38, "later frames all go to the plane: {ok}");
    assert!(log.presented >= 30, "the compositor presents the subsurface's commits: {}", log.presented);
    assert!(max_held <= 3, "frames are released as the compositor lets go: held {max_held}");
    assert!(plane.in_flight() <= 1, "all but the one on screen released: {}", plane.in_flight());
}

/// What the compositor did with the pixels: a coloured NV12 frame on the
/// plane, screenshot, and the video's colour compared with BT.601 and
/// BT.709 conversions of it. Weston 13 has no color-representation, so this
/// records which matrix it ASSUMES -- the reason `auto` scanout needs the
/// protocol (or BT.601 content) and `force` is a choice.
#[test]
fn the_compositor_shows_our_pixels_and_this_is_its_matrix() {
    use vidkit_core::frame::{DmaBufImage, DmaBufPlane, Frame, FrameData, PlaneLayout};
    use vidkit_wayland::plane::VideoPlane;
    if Command::new("weston-screenshooter").arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).status().is_err() {
        return;
    }
    let Some((weston, conn)) = weston_gl_at("vk-wlshot", &["--debug"]) else { return };
    let (globals, mut q) = registry_queue_init::<App>(&conn).unwrap();
    let qh = q.handle();
    let comp: wl_compositor::WlCompositor = globals.bind(&qh, 1..=4, ()).unwrap();
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
    let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = comp.create_surface(&qh, ());
    let xs = wm.get_xdg_surface(&surface, &qh, ());
    let _top = xs.get_toplevel(&qh, ());
    surface.commit();
    let mut app = App::default();
    while app.configured.is_none() {
        q.blocking_dispatch(&mut app).unwrap();
    }
    xs.ack_configure(app.configured.unwrap());
    let (pw, ph) = (320i32, 200i32);
    let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk-parent".as_ptr(), 0)) };
    assert_eq!(unsafe { libc::ftruncate(std::os::fd::AsRawFd::as_raw_fd(&fd), (pw * ph * 4) as i64) }, 0);
    let pool = shm.create_pool(fd.as_fd(), pw * ph * 4, &qh, ());
    let pbuf = pool.create_buffer(0, pw, ph, pw * 4, wl_shm::Format::Xrgb8888, &qh, ());
    surface.attach(Some(&pbuf), 0, 0);
    surface.commit();
    let mut plane = VideoPlane::new(&conn, &surface, false, false).unwrap();
    surface.commit();
    conn.flush().unwrap();
    let (w, h, stride) = (160u32, 96u32, 160u32);
    let (y, u, v) = (150u8, 90u8, 180u8);
    let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk-nv12".as_ptr(), 0)) };
    let mut buf = vec![y; (stride * h) as usize];
    for _ in 0..(stride * h / 4) {
        buf.extend([u, v]);
    }
    std::io::Write::write_all(&mut std::fs::File::from(fd.try_clone().unwrap()), &buf).unwrap();
    let frame = || Frame {
        data: FrameData::DmaBuf(DmaBufImage { planes: vec![DmaBufPlane { fd: fd.try_clone().unwrap(), offset: 0, stride }], modifier: 0, fourcc: vidkit_core::dmabuf::DRM_FORMAT_NV12, owner: None }),
        width: w, height: h, layout: PlaneLayout::Nv12, color: Default::default(), pts: Duration::ZERO, duration: None, recycle: None, grain: None,
    };
    let mut shown = 0;
    for _ in 0..30 {
        if plane.show(frame(), (40, 40, 240, 120)).is_ok() {
            shown += 1;
        }
        std::thread::sleep(Duration::from_millis(30));
        plane.poll();
        let _ = q.dispatch_pending(&mut app);
    }
    assert!(shown > 20);
    let shots = weston.1.join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    let ok = Command::new("weston-screenshooter")
        .current_dir(&shots)
        .env("XDG_RUNTIME_DIR", &weston.1)
        .env("WAYLAND_DISPLAY", "vk-gl")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let png_path = std::fs::read_dir(&shots).ok().and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png"))));
    let Some(png_path) = png_path.filter(|_| ok) else {
        eprintln!("DIAG no screenshot (screenshooter unavailable); pixel check skipped");
        return;
    };
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&png_path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut img = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut img).unwrap();
    let ch = info.color_type.samples();
    let px = |x: usize, yy: usize| {
        let o = (yy * info.width as usize + x) * ch;
        [img[o], img[o + 1], img[o + 2]]
    };
    // Expected colours of (150, 90, 180) limited range.
    let conv = |kr: f64, kb: f64| {
        let (yn, cb, cr) = ((y as f64 - 16.0) / 219.0, (u as f64 - 128.0) / 224.0, (v as f64 - 128.0) / 224.0);
        let kg = 1.0 - kr - kb;
        let r = yn + 2.0 * (1.0 - kr) * cr;
        let b = yn + 2.0 * (1.0 - kb) * cb;
        let g = (yn - kr * r - kb * b) / kg;
        [r, g, b].map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
    };
    let (e601, e709) = (conv(0.299, 0.114), conv(0.2126, 0.0722));
    let near = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 3);
    let mut n601 = 0;
    let mut n709 = 0;
    for yy in 0..info.height as usize {
        for x in 0..info.width as usize {
            let p = px(x, yy);
            n601 += near(p, e601) as usize;
            n709 += near(p, e709) as usize;
        }
    }
    eprintln!("DIAG screenshot {}x{}: {n601} px match BT.601 {e601:?}, {n709} match BT.709 {e709:?}", info.width, info.height);
    assert!(n601.max(n709) >= 240 * 120 / 2, "the video rectangle (240x120) is on screen in one of the two colours");
}
