//! `sim::net_channels` — multiplayer QoS channel table + per-tick byte budgeter
//! (WID r330-coordinator).
//!
//! Roadmap item 5 of `docs/sections/SECTION_multiplayer.md`; grounds the §6
//! channel layout of `handoff/MULTIPLAYER_WORLD_JOIN.md` as **pure data + a
//! budgeting helper**. A messenger needs one warm path; a game world multiplexes
//! traffic with very different QoS, so we define explicit channels over the
//! established transport, each with its own reliability/ordering and a
//! bandwidth/latency budget. The bin's transport (QUIC streams, or dhtnet
//! multiplexed ICE channels) honors `priority` / `max_bytes_per_tick`; this
//! module only *decides* the table and *allocates* a tick's byte budget across
//! channels.
//!
//! Carried-over design rules (MULTIPLAYER_WORLD_JOIN §6):
//!   - **Snapshots are unreliable-sequenced**, not reliable (a dropped transform
//!     is replaced by the next one; blocking on retransmit would spike latency).
//!   - **Bulk is throttled below snapshots** so region streaming can't starve the
//!     hot loop — modelled here by giving `Bulk` the lowest priority, so the
//!     priority-ordered budgeter only ever feeds it the leftover after the hot
//!     channels are served.
//!   - **Separate intent from bulk**: control/join is tiny and always flows; the
//!     heavy world download only runs on a warm path (the bin gates that; the
//!     budgeter just keeps `Bulk` from out-competing the hot channels).
//!
//! Pure: dependency-free plain types, no serde/clock/sockets/DHT/RNG, no
//! `.unwrap()`/`.expect()`/`panic!` in non-test code, saturating arithmetic
//! throughout. Release-safe.

/// Reliability + ordering guarantee a channel asks the transport to provide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelMode {
    /// Must not drop or reorder (handshake, inputs, world edits, chat).
    ReliableOrdered,
    /// Newest-wins; stale packets dropped, never blocks on a lost one
    /// (entity snapshots).
    UnreliableSequenced,
    /// Fire-and-forget; latency over completeness (voice).
    Unreliable,
}

/// The fixed set of multiplexed channels (MULTIPLAYER_WORLD_JOIN §6 table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelId {
    /// Handshake, join req/approve, allowlist, world meta. Reliable, tiny.
    Control,
    /// Dragon/player/mob transforms. Unreliable-sequenced, tight (<50 ms), steady.
    Snapshots,
    /// Inputs, ability casts, world edits. Reliable-ordered per-sender, tight.
    Input,
    /// Proximity voice. Unreliable, tightest latency, jitter-buffered client-side.
    Voice,
    /// Initial world chunk download / region streaming. Reliable, **throttled**.
    Bulk,
    /// Text, notifications. Reliable, loose, tiny.
    Chat,
}

impl ChannelId {
    /// All channels in declaration order (stable; not priority order).
    pub const ALL: [ChannelId; 6] = [
        ChannelId::Control,
        ChannelId::Snapshots,
        ChannelId::Input,
        ChannelId::Voice,
        ChannelId::Bulk,
        ChannelId::Chat,
    ];
}

/// Pure per-channel config the bin transport honors. Mirrors the §6 shape:
/// `{ id, mode, priority, max_bytes_per_tick, keepalive_ms }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelConfig {
    pub id: ChannelId,
    pub mode: ChannelMode,
    /// Higher = served first within a tick. `Bulk` is intentionally lowest so it
    /// only ever receives leftover budget (it can't starve the hot loop).
    pub priority: u8,
    /// Per-tick byte cap for this channel (the transport must not exceed it).
    pub max_bytes_per_tick: u32,
    /// Idle keepalive cadence in ms (0 = none; the channel relies on whatever
    /// traffic already holds the NAT mapping open). The control channel carries
    /// the punch keepalive that holds the mapping for the whole session.
    pub keepalive_ms: u32,
}

// ── Session-level cadences (§6: discovery freshness vs DHT load) ─────────────

/// World-advert re-announce cadence — discovery freshness vs DHT load (§6).
pub const ADVERT_REANNOUNCE_MS: u32 = 300_000; // ~5 min
/// DHT value TTL the re-announce loop refreshes ahead of (§6).
pub const DHT_TTL_MS: u32 = 600_000; // ~10 min
/// Punch keepalive on idle sessions, to hold the NAT mapping open (§6).
pub const PUNCH_KEEPALIVE_MS: u32 = 15_000; // ~15 s

/// The default channel table (the §6 QoS layout as concrete data). Priorities
/// place the latency-critical hot channels first and `Bulk` dead last; per-tick
/// caps assume a ~30 Hz transport tick (bin can rescale).
pub const DEFAULT_CHANNELS: [ChannelConfig; 6] = [
    ChannelConfig {
        id: ChannelId::Voice,
        mode: ChannelMode::Unreliable,
        priority: 250, // tightest latency (but small, so it can't dominate a tick)
        max_bytes_per_tick: 1024,
        keepalive_ms: 0,
    },
    ChannelConfig {
        id: ChannelId::Snapshots,
        mode: ChannelMode::UnreliableSequenced,
        priority: 200, // tight (<50 ms), steady, medium
        max_bytes_per_tick: 4096,
        keepalive_ms: 0,
    },
    ChannelConfig {
        id: ChannelId::Input,
        mode: ChannelMode::ReliableOrdered,
        priority: 190, // tight, reliable, small — must not drop/reorder
        max_bytes_per_tick: 512,
        keepalive_ms: 0,
    },
    ChannelConfig {
        id: ChannelId::Control,
        mode: ChannelMode::ReliableOrdered,
        priority: 150, // reliable, loose, tiny; carries the punch keepalive
        max_bytes_per_tick: 256,
        keepalive_ms: PUNCH_KEEPALIVE_MS,
    },
    ChannelConfig {
        id: ChannelId::Chat,
        mode: ChannelMode::ReliableOrdered,
        priority: 100, // loose, tiny
        max_bytes_per_tick: 256,
        keepalive_ms: 0,
    },
    ChannelConfig {
        id: ChannelId::Bulk,
        mode: ChannelMode::ReliableOrdered,
        priority: 10, // LOWEST — leftover only; throttled below snapshots
        max_bytes_per_tick: 16_384,
        keepalive_ms: 0,
    },
];

/// Look up the default config for a channel. Total over a fixed 6-arm table,
/// so it can never fail (no `Option`).
pub fn config_for(id: ChannelId) -> ChannelConfig {
    let mut i = 0;
    while i < DEFAULT_CHANNELS.len() {
        if DEFAULT_CHANNELS[i].id == id {
            return DEFAULT_CHANNELS[i];
        }
        i += 1;
    }
    // Unreachable: DEFAULT_CHANNELS covers every ChannelId variant (asserted in
    // tests). Return a safe inert config rather than panic, to stay release-safe.
    ChannelConfig {
        id,
        mode: ChannelMode::Unreliable,
        priority: 0,
        max_bytes_per_tick: 0,
        keepalive_ms: 0,
    }
}

/// Channels ordered highest-priority-first (the order the budgeter serves them).
/// Stable for equal priorities (insertion-sorted over the fixed table).
pub fn priority_order() -> Vec<ChannelConfig> {
    let mut out: Vec<ChannelConfig> = DEFAULT_CHANNELS.to_vec();
    // Simple stable insertion sort, descending by priority (table is tiny).
    let mut i = 1;
    while i < out.len() {
        let mut j = i;
        while j > 0 && out[j - 1].priority < out[j].priority {
            out.swap(j - 1, j);
            j -= 1;
        }
        i += 1;
    }
    out
}

/// One channel's grant for a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub id: ChannelId,
    /// Bytes this channel may send this tick (≤ its `max_bytes_per_tick` and
    /// ≤ its pending demand).
    pub bytes: u32,
}

/// Allocate a tick's byte budget across channels, honoring per-channel caps and
/// priority, so latency-critical traffic is served before `Bulk` (which only
/// gets the leftover — the §6 "bulk throttled below snapshots" rule).
///
/// `pending` is `(channel, bytes_queued)`; duplicate entries for one channel are
/// summed. `tick_budget` is the link's total byte budget for this tick. Returns
/// one `Grant` per channel that had pending demand, in priority order. The sum
/// of grants never exceeds `tick_budget`, and no grant exceeds that channel's
/// `max_bytes_per_tick`. Fully deterministic; saturating arithmetic.
pub fn budget_tick(pending: &[(ChannelId, u32)], tick_budget: u32) -> Vec<Grant> {
    let mut grants: Vec<Grant> = Vec::new();
    let mut remaining = tick_budget;

    for cfg in priority_order() {
        // Sum any pending demand for this channel (defensive against dupes).
        let mut demand: u32 = 0;
        for &(id, bytes) in pending {
            if id == cfg.id {
                demand = demand.saturating_add(bytes);
            }
        }
        if demand == 0 {
            continue; // nothing queued on this channel this tick
        }
        let cap = demand.min(cfg.max_bytes_per_tick);
        let grant = cap.min(remaining);
        remaining = remaining.saturating_sub(grant);
        grants.push(Grant {
            id: cfg.id,
            bytes: grant,
        });
    }
    grants
}

/// Total bytes across a grant set (saturating).
pub fn total_granted(grants: &[Grant]) -> u32 {
    let mut sum: u32 = 0;
    for g in grants {
        sum = sum.saturating_add(g.bytes);
    }
    sum
}

/// Whether a channel's idle keepalive is due. `keepalive_ms == 0` means the
/// channel never sends a standalone keepalive. Uses saturating elapsed math.
pub fn keepalive_due(cfg: &ChannelConfig, last_sent_ms: u64, now_ms: u64) -> bool {
    if cfg.keepalive_ms == 0 {
        return false;
    }
    now_ms.saturating_sub(last_sent_ms) >= cfg.keepalive_ms as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_every_channel_uniquely() {
        assert_eq!(DEFAULT_CHANNELS.len(), 6);
        for id in ChannelId::ALL {
            let n = DEFAULT_CHANNELS.iter().filter(|c| c.id == id).count();
            assert_eq!(n, 1, "exactly one config per channel: {id:?}");
        }
    }

    #[test]
    fn config_for_round_trips_every_id() {
        for id in ChannelId::ALL {
            assert_eq!(config_for(id).id, id);
        }
    }

    #[test]
    fn bulk_is_strictly_lowest_priority() {
        let bulk = config_for(ChannelId::Bulk).priority;
        for cfg in DEFAULT_CHANNELS {
            if cfg.id != ChannelId::Bulk {
                assert!(
                    cfg.priority > bulk,
                    "{:?} ({}) must outrank Bulk ({})",
                    cfg.id,
                    cfg.priority,
                    bulk
                );
            }
        }
    }

    #[test]
    fn priority_order_is_descending_and_complete() {
        let order = priority_order();
        assert_eq!(order.len(), 6);
        for w in order.windows(2) {
            assert!(w[0].priority >= w[1].priority, "non-increasing priority");
        }
        assert_eq!(order.last().map(|c| c.id), Some(ChannelId::Bulk));
    }

    #[test]
    fn snapshots_are_unreliable_sequenced() {
        assert_eq!(
            config_for(ChannelId::Snapshots).mode,
            ChannelMode::UnreliableSequenced
        );
        // input/control/chat/bulk must be reliable-ordered; voice unreliable.
        assert_eq!(config_for(ChannelId::Input).mode, ChannelMode::ReliableOrdered);
        assert_eq!(config_for(ChannelId::Voice).mode, ChannelMode::Unreliable);
    }

    #[test]
    fn budget_respects_per_channel_cap() {
        // Snapshots queue more than their cap; grant clamps to the cap.
        let cap = config_for(ChannelId::Snapshots).max_bytes_per_tick;
        let g = budget_tick(&[(ChannelId::Snapshots, cap + 9999)], u32::MAX);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].bytes, cap);
    }

    #[test]
    fn budget_never_exceeds_tick_budget() {
        let pending = [
            (ChannelId::Snapshots, 4096),
            (ChannelId::Input, 512),
            (ChannelId::Bulk, 16_384),
        ];
        let g = budget_tick(&pending, 2000);
        assert!(total_granted(&g) <= 2000, "sum of grants within tick budget");
    }

    #[test]
    fn bulk_only_gets_leftover_and_cannot_starve_hot_channels() {
        // Tight budget that covers the hot channels but leaves little for bulk.
        let pending = [
            (ChannelId::Snapshots, 4096),
            (ChannelId::Input, 512),
            (ChannelId::Voice, 800),
            (ChannelId::Bulk, 16_384),
        ];
        let hot_total = 4096 + 512 + 800;
        let g = budget_tick(&pending, hot_total + 100);
        let grant = |id: ChannelId| g.iter().find(|x| x.id == id).map(|x| x.bytes).unwrap_or(0);
        assert_eq!(grant(ChannelId::Snapshots), 4096, "snapshots fully served");
        assert_eq!(grant(ChannelId::Input), 512, "input fully served");
        assert_eq!(grant(ChannelId::Voice), 800, "voice fully served");
        assert_eq!(grant(ChannelId::Bulk), 100, "bulk gets only the leftover");
    }

    #[test]
    fn zero_budget_grants_nothing() {
        let g = budget_tick(&[(ChannelId::Snapshots, 4096)], 0);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].bytes, 0);
    }

    #[test]
    fn duplicate_pending_entries_are_summed_then_capped() {
        let cap = config_for(ChannelId::Input).max_bytes_per_tick;
        // Two queued chunks for Input; summed demand exceeds the cap → clamped.
        let g = budget_tick(&[(ChannelId::Input, cap - 1), (ChannelId::Input, 50)], u32::MAX);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].bytes, cap, "summed demand clamps to channel cap");
    }

    #[test]
    fn unlisted_channels_get_no_grant() {
        let g = budget_tick(&[(ChannelId::Snapshots, 100)], u32::MAX);
        assert_eq!(g.len(), 1, "only the channel with demand appears");
        assert_eq!(g[0].id, ChannelId::Snapshots);
    }

    #[test]
    fn keepalive_due_only_for_control_and_after_interval() {
        let ctrl = config_for(ChannelId::Control);
        assert!(!keepalive_due(&ctrl, 0, PUNCH_KEEPALIVE_MS as u64 - 1));
        assert!(keepalive_due(&ctrl, 0, PUNCH_KEEPALIVE_MS as u64));
        // Channels with keepalive_ms == 0 never fire.
        let snap = config_for(ChannelId::Snapshots);
        assert!(!keepalive_due(&snap, 0, u64::MAX));
    }

    #[test]
    fn budget_is_deterministic() {
        let pending = [
            (ChannelId::Bulk, 9000),
            (ChannelId::Snapshots, 4096),
            (ChannelId::Chat, 64),
        ];
        assert_eq!(budget_tick(&pending, 5000), budget_tick(&pending, 5000));
    }
}
