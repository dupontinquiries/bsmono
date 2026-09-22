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
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tideshell-output-power".into())
            .spawn(move || {
                if let Err(e) = run(rx) {
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

struct State {
    manager: ZwlrOutputPowerManagerV1,
    outputs: Vec<WlOutput>,
    qh: QueueHandle<State>,
}

fn run(rx: Receiver<bool>) -> Result<(), String> {
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
    // `Registry::bind` is infallible at the protocol level (it only sends a
    // request); a global that does not really exist is a server bug the
    // connection would eventually report some other way, not something this
    // call can detect.
    let outputs: Vec<WlOutput> = globals
        .contents()
        .with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect::<Vec<_>>())
        .into_iter()
        .map(|(name, ver)| globals.registry().bind::<WlOutput, _, _>(name, ver.min(4), &qh, ()))
        .collect();
    if outputs.is_empty() {
        return Err("no wl_output found".into());
    }
    let mut state = State { manager, outputs, qh };
    queue.roundtrip(&mut state).map_err(|e| format!("roundtrip: {e}"))?;

    loop {
        let on = rx.recv().map_err(|_| "sender dropped".to_string())?;
        let mode = decide(on);
        for output in &state.outputs {
            let power = state.manager.get_output_power(output, &state.qh, ());
            power.set_mode(mode);
        }
        // Flush the requests and let `mode`/`failed` events land (logged, not
        // acted on -- there is no fallback path to try if the compositor
        // refuses, only something worth knowing from a log).
        queue.roundtrip(&mut state).map_err(|e| format!("roundtrip: {e}"))?;
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

impl Dispatch<ZwlrOutputPowerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrOutputPowerV1, e: zwlr_output_power_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            zwlr_output_power_v1::Event::Mode { mode } => eprintln!("output power: now {mode:?}"),
            zwlr_output_power_v1::Event::Failed => eprintln!("output power: compositor refused the request"),
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
    fn on_and_off_map_to_the_protocols_two_modes() {
        assert_eq!(decide(true), Mode::On);
        assert_eq!(decide(false), Mode::Off);
    }
}
