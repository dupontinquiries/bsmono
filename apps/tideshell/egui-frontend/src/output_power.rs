//! output_power — turn the panel on or off via `wlr-output-power-management-v1`.
//!
//! ## Why its own connection
//!
//! Every other Wayland-facing piece here owns its connection: `layer/mod.rs`
//! for the strips, `wayland.rs`'s `Windows` (through systemcompkit) for
//! toplevels, `ei.rs` for the trackpad. `Action::DisplayPower` was `Unwired`
//! ("transport not wired yet") in both `effects.rs` and `bin/tideshell.rs`
//! because none of those connections is a natural place to bind one more
//! global -- `layer`'s owns the strips' own event loop, and `Windows`'s lives
//! inside systemcompkit. A dedicated connection, spawned once and driven by its
//! own thread, matches the existing shape instead of reworking either.
//!
//! ## The protocol
//!
//! `zwlr_output_power_manager_v1` (from `wayland-protocols-wlr`, already a
//! `layer`-feature dependency for the layer-shell strips) hands out one
//! `zwlr_output_power_v1` per `wl_output`, with a single request,
//! `set_mode(on | off)`, and an event, `mode` or `failed`, telling the client
//! whether the request took. cosmic-comp exposes the manager
//! (`OutputPowerState::new`, checked in its `state.rs`) to any unsandboxed
//! client.
//!
//! ## Verified and not
//!
//! `decide` -- what request a `DisplayPower` action turns into -- is pure and
//! tested. The connection, registry binding and event dispatch are real code
//! checked only by `cargo check --features layer` (no Wayland display exists in
//! this container to run it against); UNVERIFIED on a real compositor, the same
//! caveat every other direct-protocol client in this codebase already carries.

use std::sync::mpsc::{self, Receiver, Sender};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output::WlOutput, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
    zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1},
};

/// What `Action::DisplayPower(on)` becomes on the wire: pure, so it is tested
/// without a display. Kept as a function rather than inlined so a future mode
/// (e.g. DPMS "suspend" as distinct from fully off) has one place to add.
pub fn decide(on: bool) -> Mode {
    if on { Mode::On } else { Mode::Off }
}

/// A running screen-power client: send `true`/`false`, applied to every output
/// the compositor has (a phone has one).
pub struct OutputPower {
    tx: Sender<bool>,
}

impl OutputPower {
    pub fn spawn() -> OutputPower {
        Self::spawn_observing(|_| {})
    }

    /// As `spawn`, and `on_mode(on)` is called whenever the display's power
    /// actually changes -- INCLUDING changes the compositor makes itself: the
    /// lock screen and idle blanking. Before this, the power objects existed
    /// only for tideshell's own requests, so tideshell never knew the screen
    /// had gone off, and auto-brightness kept writing to a dark panel.
    pub fn spawn_observing(on_mode: impl Fn(bool) + Send + 'static) -> OutputPower {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tideshell-output-power".into())
            .spawn(move || {
                if let Err(e) = run(rx, Box::new(on_mode)) {
                    eprintln!("output power: {e}");
                }
            })
            .expect("spawn output-power thread");
        OutputPower { tx }
    }

    pub fn set(&self, on: bool) {
        let _ = self.tx.send(on);
    }
}

/// Whether the display counts as on, from each output's last reported mode:
/// on if ANY output is on, off once every output has said it is off, unknown
/// until then. Pure.
pub fn combine(modes: &[Option<bool>]) -> Option<bool> {
    if modes.iter().any(|m| *m == Some(true)) {
        Some(true)
    } else if !modes.is_empty() && modes.iter().all(|m| *m == Some(false)) {
        Some(false)
    } else {
        None
    }
}

struct State {
    /// Last reported mode per output, by index.
    modes: Vec<Option<bool>>,
    /// What was last passed to `on_mode`, so it fires on CHANGES only.
    reported: Option<bool>,
    on_mode: Box<dyn Fn(bool) + Send>,
}

fn run(rx: Receiver<bool>, on_mode: Box<dyn Fn(bool) + Send>) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("connect: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();
    let manager: ZwlrOutputPowerManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|_| "no zwlr_output_power_manager_v1 -- compositor does not support screen power")?;
    // ALL outputs, not just the first: `GlobalList::bind` gives one instance of
    // an interface, but a phone with an external display (or, later, a
    // multi-panel device) has more than one `wl_output`, and every one of them
    // should follow the request.
    let outputs: Vec<WlOutput> = globals
        .contents()
        .with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect::<Vec<_>>())
        .into_iter()
        .map(|(name, ver)| globals.registry().bind::<WlOutput, _, _>(name, ver.min(4), &qh, ()))
        .collect();
    if outputs.is_empty() {
        return Err("no wl_output found".into());
    }
    // One power object per output, kept for the life of the connection: the
    // protocol reports its mode once on creation and again on EVERY change,
    // whoever made it. (They used to be created per request, and dropped.)
    let powers: Vec<ZwlrOutputPowerV1> =
        outputs.iter().enumerate().map(|(i, o)| manager.get_output_power(o, &qh, i)).collect();
    let mut state = State { modes: vec![None; powers.len()], reported: None, on_mode };

    // tideshell's own requests go out from a second thread -- proxies are
    // thread-safe and the connection can be flushed from anywhere -- so THIS
    // one can block in dispatch and hear every mode change as it happens.
    let (req_powers, req_conn) = (powers.clone(), conn.clone());
    std::thread::Builder::new()
        .name("tideshell-output-power-req".into())
        .spawn(move || {
            while let Ok(on) = rx.recv() {
                for p in &req_powers {
                    p.set_mode(decide(on));
                }
                let _ = req_conn.flush();
            }
        })
        .map_err(|e| format!("request thread: {e}"))?;
    loop {
        queue.blocking_dispatch(&mut state).map_err(|e| format!("dispatch: {e}"))?;
    }
}

macro_rules! noop {
    ($t:ty) => {
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    };
}
noop!(ZwlrOutputPowerManagerV1);
noop!(WlOutput);

impl Dispatch<ZwlrOutputPowerV1, usize> for State {
    fn event(state: &mut Self, _: &ZwlrOutputPowerV1, e: zwlr_output_power_v1::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match e {
            zwlr_output_power_v1::Event::Mode { mode } => {
                let on = matches!(mode, wayland_client::WEnum::Value(Mode::On));
                eprintln!("output power: output {idx} now {}", if on { "on" } else { "off" });
                if let Some(m) = state.modes.get_mut(*idx) {
                    *m = Some(on);
                }
                let now = combine(&state.modes);
                if now.is_some() && now != state.reported {
                    state.reported = now;
                    (state.on_mode)(now.unwrap());
                }
            }
            zwlr_output_power_v1::Event::Failed => eprintln!("output power: compositor refused (output {idx})"),
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_is_on_if_any_output_is_and_off_only_when_all_say_so() {
        assert_eq!(combine(&[Some(true)]), Some(true));
        assert_eq!(combine(&[Some(false)]), Some(false));
        assert_eq!(combine(&[Some(false), None]), None, "not all known yet: no claim");
        assert_eq!(combine(&[Some(false), Some(true)]), Some(true), "an external screen still on");
        assert_eq!(combine(&[]), None);
    }

    #[test]
    fn on_and_off_map_to_the_protocols_two_modes() {
        assert_eq!(decide(true), Mode::On);
        assert_eq!(decide(false), Mode::Off);
    }
}
