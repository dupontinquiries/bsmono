// src/mesh_runner.rs
//
// Runs the Mesh (mesh.rs) on its own thread and gives the daemon a simple
// handle: send commands in, read a shared snapshot out. This is the glue
// that turns the verified election + the UDP transport into something the
// daemon's start_session / leave_session / list_peers commands drive.
//
// The Mesh itself does blocking socket reads with a short timeout in tick(),
// so the runner just loops tick() and drains an inbound command channel
// between ticks. Outbound MeshEvents are folded into a shared snapshot
// (peers, role, last assignment) that the daemon reads under a mutex —
// cheap, since the control loop is low-frequency.

use crate::mesh::{Mesh, MeshEvent};
use crate::protocol::{NodeInfo, Layout, Claim, Channel};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Commands the daemon sends the mesh thread.
pub enum MeshCmd {
    StartSession(Layout),
    StopSession,
    /// Pin a channel to a speaker by user request (drag-to-speaker).
    PinClaim { channel: crate::protocol::Channel, speaker: crate::protocol::SpeakerId },
    Shutdown,
}

/// Snapshot of mesh state the daemon can read at any time.
#[derive(Default, Clone)]
pub struct MeshSnapshot {
    pub role: String,
    pub peers: Vec<NodeInfo>,
    pub assignment: Vec<Claim>,
    pub unclaimed: Vec<Channel>,
    pub last_start: Option<(u64, String)>, // (start_at_ms, channel_base_url)
}

/// Handle the daemon keeps. Drop it (Shutdown) to stop the mesh thread.
pub struct MeshHandle {
    cmd_tx: Sender<MeshCmd>,
    snapshot: Arc<Mutex<MeshSnapshot>>,
}

impl MeshHandle {
    pub fn start_session(&self, layout: Layout) {
        let _ = self.cmd_tx.send(MeshCmd::StartSession(layout));
    }
    pub fn leave_session(&self) {
        let _ = self.cmd_tx.send(MeshCmd::StopSession);
    }
    /// Pin a channel to a speaker (drag-to-speaker from the UI).
    pub fn pin_claim(&self, channel: crate::protocol::Channel,
                     speaker: crate::protocol::SpeakerId) {
        let _ = self.cmd_tx.send(MeshCmd::PinClaim { channel, speaker });
    }
    pub fn shutdown(&self) {
        let _ = self.cmd_tx.send(MeshCmd::Shutdown);
    }
    /// Read the current snapshot (clones — cheap, small).
    pub fn snapshot(&self) -> MeshSnapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }
    /// Peers as a JSON array string for the API.
    pub fn peers_json(&self) -> String {
        let snap = self.snapshot();
        serde_json::to_string(&snap.peers).unwrap_or_else(|_| "[]".into())
    }
    pub fn role(&self) -> String { self.snapshot().role }

    /// Full fabric state for the drag-to-speaker UI: every peer with the
    /// speakers it can drive, the current channel→speaker assignment,
    /// and any unclaimed channels. One JSON blob so the controller polls
    /// it in a single round-trip.
    pub fn fabric_json(&self) -> String {
        let snap = self.snapshot();
        serde_json::json!({
            "role": snap.role,
            "peers": snap.peers,
            "assignment": snap.assignment,
            "unclaimed": snap.unclaimed,
        }).to_string()
    }
}

/// Spawn the mesh on a thread. Returns a handle, or an error if the socket
/// can't be opened (e.g. multicast not permitted) — in which case the daemon
/// runs single-device and reports role "idle".
pub fn spawn(me: NodeInfo) -> std::io::Result<MeshHandle> {
    let (cmd_tx, cmd_rx): (Sender<MeshCmd>, Receiver<MeshCmd>) = channel();
    let (evt_tx, evt_rx) = channel::<MeshEvent>();
    let snapshot = Arc::new(Mutex::new(MeshSnapshot { role: "idle".into(), ..Default::default() }));

    // Construct the mesh up front so socket errors surface here (not on the
    // thread, where the daemon couldn't observe them).
    let mesh = Mesh::new(me, evt_tx)?;

    let snap_for_thread = snapshot.clone();
    thread::spawn(move || run(mesh, cmd_rx, evt_rx, snap_for_thread));

    Ok(MeshHandle { cmd_tx, snapshot })
}

fn run(
    mut mesh: Mesh,
    cmd_rx: Receiver<MeshCmd>,
    evt_rx: Receiver<MeshEvent>,
    snapshot: Arc<Mutex<MeshSnapshot>>,
) {
    loop {
        // 1. Drain inbound commands.
        loop {
            match cmd_rx.try_recv() {
                Ok(MeshCmd::StartSession(layout)) => mesh.start_session(layout),
                Ok(MeshCmd::StopSession) => mesh.stop_session(),
                Ok(MeshCmd::PinClaim { channel, speaker }) => mesh.pin_claim(channel, speaker),
                Ok(MeshCmd::Shutdown) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }

        // 2. Advance the mesh (reads sockets w/ timeout, sends heartbeats,
        //    resolves assignments, emits MeshEvents).
        mesh.tick();

        // 3. Fold any emitted events into the shared snapshot.
        let mut changed = false;
        loop {
            match evt_rx.try_recv() {
                Ok(ev) => { apply_event(&snapshot, ev); changed = true; }
                Err(_) => break,
            }
        }
        let _ = changed;

        // 4. Small sleep so we don't spin (tick already blocks ~200ms on the
        //    socket read timeout, so this is just a floor).
        thread::sleep(Duration::from_millis(20));
    }
}

fn apply_event(snapshot: &Arc<Mutex<MeshSnapshot>>, ev: MeshEvent) {
    if let Ok(mut s) = snapshot.lock() {
        match ev {
            MeshEvent::PeersChanged(peers) => s.peers = peers,
            MeshEvent::RoleChanged(role) => s.role = role,
            MeshEvent::Assignment(map, unclaimed) => { s.assignment = map; s.unclaimed = unclaimed; }
            MeshEvent::StartPlayback { start_at_ms, channel_base_url, .. } => {
                s.last_start = Some((start_at_ms, channel_base_url));
            }
            MeshEvent::Stopped => { s.role = "idle".into(); s.assignment.clear(); }
            // Pass-by canceller routing — these events bubble through
            // to the bo-muxd binary's main loop (which owns the
            // MicCapture + PassbyCanceller); apply_event's job is
            // only to keep the MeshSnapshot fresh. We don't add a
            // dedicated snapshot field yet (the GUI accesses the
            // controller's own runner state for now); this is
            // routing-only.
            MeshEvent::PassbyStandbyStart { .. } => {}
            MeshEvent::PassbyStandbyStop => {}
            MeshEvent::PassbyStatus { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Platform, Speaker};

    fn node(id: &str) -> NodeInfo {
        NodeInfo {
            node_id: id.into(), name: id.into(), platform: Platform::Linux,
            speakers: vec![Speaker { id: "local".into(), name: "out".into(), platform: Platform::Linux }],
        }
    }

    #[test]
    fn snapshot_starts_idle() {
        // We can't bind multicast in all CI sandboxes; just exercise the
        // snapshot type + apply_event logic without a live socket.
        let snap = Arc::new(Mutex::new(MeshSnapshot { role: "idle".into(), ..Default::default() }));
        apply_event(&snap, MeshEvent::RoleChanged("coordinator".into()));
        assert_eq!(snap.lock().unwrap().role, "coordinator");
    }

    #[test]
    fn peers_fold_into_snapshot() {
        let snap = Arc::new(Mutex::new(MeshSnapshot::default()));
        apply_event(&snap, MeshEvent::PeersChanged(vec![node("a"), node("b")]));
        assert_eq!(snap.lock().unwrap().peers.len(), 2);
    }

    #[test]
    fn stopped_resets_role_and_assignment() {
        let snap = Arc::new(Mutex::new(MeshSnapshot {
            role: "coordinator".into(),
            assignment: vec![Claim { node_id: "a".into(), channel: "FL".into(),
                speaker: "local".into(), cost: 1 }],
            ..Default::default()
        }));
        apply_event(&snap, MeshEvent::Stopped);
        let s = snap.lock().unwrap();
        assert_eq!(s.role, "idle");
        assert!(s.assignment.is_empty());
    }
}
