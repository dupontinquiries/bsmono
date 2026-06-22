// src/protocol.rs
//
// The coordination protocol for distributed multi-speaker playback.
//
// This module is pure logic — message types, the term-based "latest
// announcer takes over" election, and the channel-assignment resolver.
// No I/O, no threads, so it's fully unit-testable (see tests at bottom).
// mesh.rs wires these messages to the wire; this file decides what they
// mean.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type NodeId = String;     // stable per-install uuid
pub type Channel = String;    // "FL","FR","C","LFE","SL","SR" or "water","birds",...
pub type SpeakerId = String;  // url for B&O, or "local" for this device's PipeWire

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Platform { Mozart, Ase, Linux, Phone, Other }

/// A speaker some node can drive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: SpeakerId,        // "http://192.168.2.12" or "local"
    pub name: String,
    pub platform: Platform,
}

/// What a node advertises about itself on the mesh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub node_id: NodeId,
    pub name: String,
    pub platform: Platform,
    pub speakers: Vec<Speaker>,   // speakers this node can drive
}

/// The layout the coordinator wants for the current content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub name: String,                 // "5.1", "quad", "ambient:forest"
    pub channels: Vec<Channel>,
}

impl Layout {
    pub fn stereo() -> Self { Self { name: "2.0".into(), channels: vec!["L".into(), "R".into()] } }
    pub fn quad() -> Self {
        Self { name: "quad".into(), channels: ["FL","FR","RL","RR"].iter().map(|s| s.to_string()).collect() }
    }
    pub fn surround_51() -> Self {
        Self { name: "5.1".into(), channels: ["FL","FR","C","LFE","SL","SR"].iter().map(|s| s.to_string()).collect() }
    }
}

/// A node's claim to drive a specific channel on a specific speaker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub node_id: NodeId,
    pub channel: Channel,
    pub speaker: SpeakerId,
    /// Lower = preferred (e.g. link quality metric, or 0 if user-pinned).
    pub cost: u32,
}

/// The wire messages exchanged on the mesh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// Periodic presence broadcast (who I am, what I can drive).
    Hello(NodeInfo),
    /// Start/take over a session. Higher term wins.
    Announce { term: u64, coordinator: NodeId, layout: Layout },
    /// Coordinator keep-alive.
    Heartbeat { term: u64, coordinator: NodeId },
    /// A member offering to drive a channel.
    ClaimChannel { term: u64, claim: Claim },
    /// Coordinator's resolved final assignment.
    Assign { term: u64, map: Vec<Claim> },
    /// Coordinator tells everyone to begin playout at a scheduled time.
    /// `start_at_ms` is in coordinator-clock epoch millis.
    Start { term: u64, start_at_ms: u64, channel_base_url: String },
    /// Stop the current session.
    Stop { term: u64 },
    /// GUI → mux: enable overnight pass-by cancellation. The mux node
    /// receiving this starts watching its own mic + emits anti-noise
    /// to the sink in `sink_name`. Honored only when authenticated;
    /// unauthenticated peers can't trigger it.
    StartPassbyStandby { sink_name: String },
    /// GUI → mux: stop overnight pass-by cancellation.
    StopPassbyStandby,
    /// Mux → GUI: status update from a node running standby mode.
    /// Broadcast at ~1 Hz while standby is active.
    PassbyStatus {
        node_id: NodeId,
        phase: String,
        fundamental_hz: f32,
        doppler_hz_per_s: f32,
        timer_confidence: f32,
    },
}

/// Local role state machine for one node.
#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Idle,
    Coordinator { term: u64, layout: Layout },
    Member { term: u64, coordinator: NodeId },
}

/// Tracks election state for this node. Pure; feed it incoming messages
/// and local intents, read out the resulting role + any message to send.
#[derive(Debug, Clone)]
pub struct Election {
    pub me: NodeId,
    pub role: Role,
    pub highest_term_seen: u64,
}

impl Election {
    pub fn new(me: NodeId) -> Self {
        Self { me, role: Role::Idle, highest_term_seen: 0 }
    }

    /// Local intent: this node wants to start (or take over) a session.
    /// Returns the Announce to broadcast.
    pub fn announce(&mut self, layout: Layout) -> Msg {
        let term = self.highest_term_seen + 1;
        self.highest_term_seen = term;
        self.role = Role::Coordinator { term, layout: layout.clone() };
        Msg::Announce { term, coordinator: self.me.clone(), layout }
    }

    /// Handle an incoming Announce. "Latest announcer takes over":
    /// a strictly higher term always wins; equal term breaks by lower id.
    /// Returns true if our role changed.
    pub fn on_announce(&mut self, term: u64, coordinator: &NodeId, layout: &Layout) -> bool {
        let supersedes = term > self.highest_term_seen
            || (term == self.current_term() && coordinator < &self.me && coordinator != &self.me);

        if term >= self.highest_term_seen {
            self.highest_term_seen = term;
        }

        if coordinator == &self.me {
            return false; // it's our own announce echoed back
        }

        if supersedes {
            self.role = Role::Member { term, coordinator: coordinator.clone() };
            let _ = layout;
            true
        } else {
            false
        }
    }

    /// Handle a heartbeat: refreshes liveness; also catches us up if we
    /// somehow missed the Announce for a higher term.
    pub fn on_heartbeat(&mut self, term: u64, coordinator: &NodeId) -> bool {
        if term > self.highest_term_seen && coordinator != &self.me {
            self.highest_term_seen = term;
            self.role = Role::Member { term, coordinator: coordinator.clone() };
            true
        } else {
            false
        }
    }

    /// Coordinator died (missed heartbeats). Drop to Idle so the next
    /// announce (possibly our own) can take over.
    pub fn on_coordinator_lost(&mut self) {
        if matches!(self.role, Role::Member { .. }) {
            self.role = Role::Idle;
        }
    }

    fn current_term(&self) -> u64 {
        match &self.role {
            Role::Idle => 0,
            Role::Coordinator { term, .. } => *term,
            Role::Member { term, .. } => *term,
        }
    }

    pub fn is_coordinator(&self) -> bool {
        matches!(self.role, Role::Coordinator { .. })
    }
}

/// Resolve a set of channel claims into a final assignment map.
///
/// Rules:
///   - one node per channel (lowest cost wins; tie → lowest node_id)
///   - a node may hold multiple channels (e.g. bookshelf pair = FL+FR)
///   - channels with no claim are returned in `unclaimed` so the caller
///     can decide to downmix/drop/fallback.
pub fn resolve_assignments(
    layout: &Layout,
    claims: &[Claim],
) -> (Vec<Claim>, Vec<Channel>) {
    let mut best: HashMap<Channel, Claim> = HashMap::new();

    for c in claims {
        if !layout.channels.contains(&c.channel) { continue; }
        match best.get(&c.channel) {
            None => { best.insert(c.channel.clone(), c.clone()); }
            Some(existing) => {
                let better = c.cost < existing.cost
                    || (c.cost == existing.cost && c.node_id < existing.node_id);
                if better { best.insert(c.channel.clone(), c.clone()); }
            }
        }
    }

    let mut assigned = Vec::new();
    let mut unclaimed = Vec::new();
    for ch in &layout.channels {
        match best.remove(ch) {
            Some(claim) => assigned.push(claim),
            None => unclaimed.push(ch.clone()),
        }
    }
    (assigned, unclaimed)
}

/// Compute sensible default channel assignments for a small mesh
/// (2-3 speakers) when no explicit claims arrived. This is what
/// makes "plug in 2-3 BT speakers and they Just Work in stereo or
/// LCR" possible without the user manually assigning channels.
///
/// Inputs:
///   - `layout`: the layout the coordinator announced. Determines
///     the set of channels we can assign to.
///   - `peers`: the active mesh peers (Hello-known). Ordered as
///     the caller passes them; the function uses index 0 as the
///     "front-left preference" anchor and goes from there.
///
/// Returns a Vec<Claim> with sensible defaults. Each peer gets
/// `local` as its speaker_id (driving its own audio output), cost
/// 100 so explicit claims (cost < 100 by convention) override it.
///
/// Rules:
///   1 peer  → assigned to the FIRST channel of the layout (stereo: L)
///   2 peers → spread across L/R, FL/FR, etc. — the front pair
///             of the layout
///   3 peers → 2 for the front pair, 1 for center (5.1) or rear
///             (quad)
///   N > 3   → assign each peer to one channel of the layout in
///             order; extra peers don't get auto-assigned (the
///             user probably has a specific configuration in mind
///             and we shouldn't guess).
///
/// The function never returns claims for layouts it can't sensibly
/// fill — empty Vec means "the auto-assigner doesn't have an
/// opinion; require explicit claims."
pub fn smart_default_assignments(
    layout: &Layout,
    peers: &[NodeInfo],
) -> Vec<Claim> {
    if peers.is_empty() { return Vec::new(); }
    let channels = &layout.channels;
    if channels.is_empty() { return Vec::new(); }

    let mut result = Vec::new();
    let n_peers = peers.len();

    // Pick the channel set for this peer count. Different layouts
    // prefer different channel mappings — e.g. for 3 peers + a 5.1
    // layout, we want FL/FR/C (a true LCR setup); for 3 peers + a
    // quad layout, we want FL/FR/RR (a fronts-plus-rear setup)
    // because quad doesn't have a center.
    let preferred: Vec<&str> = match (n_peers, layout.name.as_str()) {
        (1, _) => channels.iter().take(1).map(|s| s.as_str()).collect(),
        (2, "2.0") => vec!["L", "R"],
        (2, _)     => vec!["FL", "FR"],
        (3, "5.1") | (3, "5_1") => vec!["FL", "FR", "C"],
        (3, "quad") => vec!["FL", "FR", "RR"],
        (3, _) => {
            // Generic 3-peer: front pair + first non-front channel
            let mut v = vec!["FL", "FR"];
            for ch in channels {
                if ch != "FL" && ch != "FR" {
                    v.push(ch.as_str());
                    break;
                }
            }
            v
        }
        _ => {
            // 4+ peers: assign one channel per peer in layout order,
            // capped at peer count.
            channels.iter().take(n_peers).map(|s| s.as_str()).collect()
        }
    };

    // Map each peer (in order) to the corresponding preferred
    // channel. If the preferred channel isn't in the layout, fall
    // back to whatever the layout has at that index.
    for (i, peer) in peers.iter().enumerate() {
        let ch = if i < preferred.len()
            && channels.iter().any(|c| c == preferred[i])
        {
            preferred[i].to_string()
        } else if i < channels.len() {
            channels[i].clone()
        } else {
            // No room in the layout for this peer.
            continue;
        };
        result.push(Claim {
            node_id: peer.node_id.clone(),
            channel: ch,
            // Each peer drives its own local audio output by
            // default. If the peer has named speakers it could
            // drive (via NodeInfo.speakers), the caller can
            // override the speaker id post-hoc.
            speaker: "local".to_string(),
            // Cost 100 — explicit user-pinned claims use cost 0
            // and override us. The mesh's resolve_assignments
            // picks the lower cost, so this lets us provide a
            // sensible default WITHOUT preventing the user from
            // overriding it.
            cost: 100,
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(node: &str, ch: &str, spk: &str, cost: u32) -> Claim {
        Claim { node_id: node.into(), channel: ch.into(), speaker: spk.into(), cost }
    }

    #[test]
    fn higher_term_takes_over() {
        let mut e = Election::new("A".into());
        // A announces term 1, becomes coordinator
        let _ = e.announce(Layout::stereo());
        assert!(e.is_coordinator());
        // B announces term 2 — A must yield
        let changed = e.on_announce(2, &"B".to_string(), &Layout::quad());
        assert!(changed);
        assert_eq!(e.role, Role::Member { term: 2, coordinator: "B".into() });
    }

    #[test]
    fn own_announce_echo_is_ignored() {
        let mut e = Election::new("A".into());
        let _ = e.announce(Layout::stereo());
        let changed = e.on_announce(1, &"A".to_string(), &Layout::stereo());
        assert!(!changed);
        assert!(e.is_coordinator());
    }

    #[test]
    fn lower_term_does_not_take_over() {
        let mut e = Election::new("A".into());
        let _ = e.on_announce(5, &"B".to_string(), &Layout::quad()); // we're member of term 5
        let changed = e.on_announce(3, &"C".to_string(), &Layout::stereo());
        assert!(!changed);
        assert_eq!(e.role, Role::Member { term: 5, coordinator: "B".into() });
    }

    #[test]
    fn heartbeat_catches_up_missed_term() {
        let mut e = Election::new("A".into());
        let changed = e.on_heartbeat(7, &"Z".to_string());
        assert!(changed);
        assert_eq!(e.highest_term_seen, 7);
    }

    #[test]
    fn coordinator_lost_drops_to_idle() {
        let mut e = Election::new("A".into());
        e.on_announce(2, &"B".to_string(), &Layout::quad());
        e.on_coordinator_lost();
        assert_eq!(e.role, Role::Idle);
    }

    #[test]
    fn assignment_lowest_cost_wins() {
        let layout = Layout::quad(); // FL FR RL RR
        let claims = vec![
            claim("phone", "RL", "local", 10),
            claim("laptop", "RL", "local", 3),   // cheaper → wins RL
            claim("stage", "FL", "http://x", 1),
            claim("stage", "FR", "http://x", 1),
        ];
        let (assigned, unclaimed) = resolve_assignments(&layout, &claims);
        let rl = assigned.iter().find(|c| c.channel == "RL").unwrap();
        assert_eq!(rl.node_id, "laptop");
        assert_eq!(unclaimed, vec!["RR".to_string()]); // nobody claimed RR
    }

    #[test]
    fn assignment_tie_breaks_by_node_id() {
        let layout = Layout::stereo();
        let claims = vec![
            claim("B", "L", "local", 5),
            claim("A", "L", "local", 5),   // same cost, lower id → wins
        ];
        let (assigned, _) = resolve_assignments(&layout, &claims);
        assert_eq!(assigned.iter().find(|c| c.channel == "L").unwrap().node_id, "A");
    }

    // ── smart_default_assignments tests ──────────────────────────

    fn peer(id: &str) -> NodeInfo {
        NodeInfo {
            node_id: id.to_string(),
            name: id.to_string(),
            platform: Platform::Linux,
            speakers: vec![],
        }
    }

    #[test]
    fn auto_assign_empty_peers_returns_empty() {
        let layout = Layout::stereo();
        let result = smart_default_assignments(&layout, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn auto_assign_two_peers_stereo_gets_L_R() {
        let layout = Layout::stereo();
        let peers = vec![peer("A"), peer("B")];
        let result = smart_default_assignments(&layout, &peers);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].channel, "L");
        assert_eq!(result[0].node_id, "A");
        assert_eq!(result[1].channel, "R");
        assert_eq!(result[1].node_id, "B");
    }

    #[test]
    fn auto_assign_two_peers_quad_gets_FL_FR() {
        // 2 peers + quad layout: prefer FL/FR front pair, not the
        // canonical layout order (which would be FL/FR — same here,
        // but the logic is layout-aware).
        let layout = Layout::quad();
        let peers = vec![peer("A"), peer("B")];
        let result = smart_default_assignments(&layout, &peers);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].channel, "FL");
        assert_eq!(result[1].channel, "FR");
    }

    #[test]
    fn auto_assign_three_peers_5_1_gets_LCR() {
        let layout = Layout::surround_51();
        let peers = vec![peer("A"), peer("B"), peer("C")];
        let result = smart_default_assignments(&layout, &peers);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].channel, "FL");
        assert_eq!(result[1].channel, "FR");
        assert_eq!(result[2].channel, "C");
    }

    #[test]
    fn auto_assign_three_peers_quad_uses_FL_FR_RR() {
        // No center in quad — pick a rear instead so 3-peer setups
        // get spatial spread.
        let layout = Layout::quad();
        let peers = vec![peer("A"), peer("B"), peer("C")];
        let result = smart_default_assignments(&layout, &peers);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].channel, "FL");
        assert_eq!(result[1].channel, "FR");
        assert_eq!(result[2].channel, "RR");
    }

    #[test]
    fn auto_assign_one_peer_takes_first_channel() {
        let layout = Layout::stereo();
        let result = smart_default_assignments(&layout, &[peer("A")]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].channel, "L");
    }

    #[test]
    fn auto_assign_uses_local_as_speaker_id() {
        let layout = Layout::stereo();
        let result = smart_default_assignments(&layout, &[peer("A")]);
        assert_eq!(result[0].speaker, "local");
    }

    #[test]
    fn auto_assign_uses_high_cost_so_explicit_claims_override() {
        let layout = Layout::stereo();
        let result = smart_default_assignments(&layout, &[peer("A")]);
        // Cost 100 — explicit user claims at cost 0-50 should win.
        assert_eq!(result[0].cost, 100);
        // Verify the merge: an explicit claim at lower cost wins.
        let mut combined: Vec<Claim> = result.clone();
        combined.push(Claim {
            node_id: "B".into(), channel: "L".into(),
            speaker: "http://my-speaker".into(), cost: 0,
        });
        let (assigned, _) = resolve_assignments(&layout, &combined);
        let l = assigned.iter().find(|c| c.channel == "L").unwrap();
        assert_eq!(l.node_id, "B",
            "explicit user-pinned claim must beat the auto-assignment");
    }

    #[test]
    fn auto_assign_four_peers_one_per_layout_channel() {
        // 4 peers + quad → one peer per channel in layout order.
        let layout = Layout::quad();
        let peers = vec![peer("A"), peer("B"), peer("C"), peer("D")];
        let result = smart_default_assignments(&layout, &peers);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].channel, "FL");
        assert_eq!(result[1].channel, "FR");
        assert_eq!(result[2].channel, "RL");
        assert_eq!(result[3].channel, "RR");
    }

    #[test]
    fn auto_assign_five_peers_quad_only_uses_four() {
        // 5 peers + 4-channel layout: 4 get assigned, the 5th is
        // skipped (returns no claim). The "extra" peer can act as
        // a backup; the user can explicitly claim if desired.
        let layout = Layout::quad();
        let peers = vec![
            peer("A"), peer("B"), peer("C"), peer("D"), peer("E"),
        ];
        let result = smart_default_assignments(&layout, &peers);
        assert_eq!(result.len(), 4);
    }
}
