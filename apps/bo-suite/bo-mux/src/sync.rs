// src/sync.rs
//
// Best-effort time synchronization across heterogeneous devices. See the
// "Sync reality" section of DESIGN.md for why this is best-effort and not
// sample-accurate: we don't own the speakers' clocks or buffers.
//
// Two pieces, both pure/testable:
//   1. NTP-style clock-offset estimation (round-trip halving) so members
//      can translate coordinator-time into local-time.
//   2. Scheduled-start math: pick a future start instant far enough out
//      that the slowest endpoint can buffer, then compute each endpoint's
//      local fire time accounting for clock offset + measured playout
//      latency.

/// One NTP-style probe exchange. All times in epoch milliseconds.
///   t0 = client send time (client clock)
///   t1 = server receive time (server clock)
///   t2 = server send time (server clock)
///   t3 = client receive time (client clock)
#[derive(Debug, Clone, Copy)]
pub struct Probe { pub t0: i64, pub t1: i64, pub t2: i64, pub t3: i64 }

impl Probe {
    /// Estimated offset of the *server* clock relative to the *client*:
    /// add this to a client timestamp to get the server's notion of it.
    ///   offset = ((t1 - t0) + (t2 - t3)) / 2
    pub fn offset_ms(&self) -> i64 {
        ((self.t1 - self.t0) + (self.t2 - self.t3)) / 2
    }
    /// Round-trip delay; lower = more trustworthy probe.
    ///   delay = (t3 - t0) - (t2 - t1)
    pub fn round_trip_ms(&self) -> i64 {
        (self.t3 - self.t0) - (self.t2 - self.t1)
    }
}

/// Combine several probes into a robust offset estimate. We take the offset
/// from the probe with the *smallest* round-trip (least queuing noise) —
/// the standard NTP "best sample" heuristic — rather than averaging, which
/// is biased by occasional long round-trips.
pub fn best_offset(probes: &[Probe]) -> Option<i64> {
    probes.iter()
        .min_by_key(|p| p.round_trip_ms().abs())
        .map(|p| p.offset_ms())
}

/// Per-endpoint timing profile.
#[derive(Debug, Clone, Copy)]
pub struct EndpointTiming {
    /// Offset of THIS endpoint's clock relative to the coordinator
    /// (add to coordinator-time to get endpoint-local time). For an
    /// endpoint we can't probe (a dumb B&O speaker), this is 0 and we
    /// rely entirely on latency below.
    pub clock_offset_ms: i64,
    /// Measured/calibrated playout latency: how long after we issue the
    /// "play" command/stream until sound actually emerges. Networked B&O
    /// speakers: ~300–2000ms. Local PipeWire: ~tens of ms.
    pub playout_latency_ms: i64,
}

/// Given the coordinator's chosen common start instant (in coordinator
/// epoch ms) and an endpoint's timing, return the local instant at which
/// THAT endpoint should *issue its play action* so that sound emerges at
/// the common instant.
///
///   issue_local = (start_coord + offset_to_local) - playout_latency
///
/// If the result is in the past relative to `now_local`, the start instant
/// was chosen too soon for this endpoint (see choose_start_instant).
pub fn local_issue_time(start_coord_ms: i64, t: &EndpointTiming) -> i64 {
    (start_coord_ms + t.clock_offset_ms) - t.playout_latency_ms
}

/// Choose a common start instant far enough in the future that EVERY
/// endpoint can issue its play action in time, given the worst-case
/// playout latency plus a safety margin.
///
///   start = now_coord + max_playout_latency + margin
///
/// Returns coordinator epoch ms.
pub fn choose_start_instant(
    now_coord_ms: i64,
    endpoints: &[EndpointTiming],
    margin_ms: i64,
) -> i64 {
    let max_latency = endpoints.iter()
        .map(|e| e.playout_latency_ms)
        .max()
        .unwrap_or(0);
    now_coord_ms + max_latency + margin_ms
}

/// Drift trim: given the measured audible offset of an endpoint from the
/// reference (positive = this endpoint is LATE), return a small playback-
/// rate nudge (ppm) to gently pull it back without an audible pitch jump.
/// Capped so corrections stay inaudible (<= ~50 ppm ≈ 0.005%).
///
/// NaN-defended: a non-finite measurement (e.g. probe timeout that fed
/// through as f32::NAN, or a window underflow that produced Inf) would
/// have propagated through clamp() — f32::clamp(NaN, lo, hi) returns
/// NaN, which then multiplies into the speaker's playback-rate nudge
/// and silently kills its audio. We return 0 (no correction) for any
/// non-finite measurement: better to skip this correction window than
/// to poison the playback rate.
pub fn drift_correction_ppm(measured_late_ms: f32, window_ms: f32) -> f32 {
    if !measured_late_ms.is_finite() || !window_ms.is_finite() { return 0.0; }
    if window_ms <= 0.0 { return 0.0; }
    // ppm needed to erase `measured_late_ms` over `window_ms`:
    let raw = (measured_late_ms / window_ms) * 1_000_000.0;
    if !raw.is_finite() { return 0.0; }
    raw.clamp(-50.0, 50.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_zero_when_symmetric() {
        // server clock identical to client, 20ms each way
        let p = Probe { t0: 1000, t1: 1020, t2: 1020, t3: 1040 };
        assert_eq!(p.offset_ms(), 0);
        assert_eq!(p.round_trip_ms(), 40);
    }

    #[test]
    fn offset_detects_server_ahead() {
        // server clock is +100ms ahead; 10ms each way
        // t0=1000 (client), t1=1110 (server: 1010+100), t2=1110, t3=1020
        let p = Probe { t0: 1000, t1: 1110, t2: 1110, t3: 1020 };
        assert_eq!(p.offset_ms(), 100);
    }

    #[test]
    fn best_offset_picks_lowest_rtt() {
        let noisy = Probe { t0: 0, t1: 200, t2: 200, t3: 500 };   // rtt 500
        let clean = Probe { t0: 0, t1: 115, t2: 115, t3: 20 };    // rtt 20, offset 105
        assert_eq!(best_offset(&[noisy, clean]), Some(105));
    }

    #[test]
    fn start_instant_clears_worst_latency() {
        let eps = [
            EndpointTiming { clock_offset_ms: 0, playout_latency_ms: 1500 }, // slow B&O
            EndpointTiming { clock_offset_ms: 0, playout_latency_ms: 40 },   // local
        ];
        let start = choose_start_instant(10_000, &eps, 250);
        assert_eq!(start, 10_000 + 1500 + 250);
    }

    #[test]
    fn local_issue_accounts_for_latency_and_offset() {
        // endpoint clock +100ms vs coordinator, 1500ms playout latency
        let t = EndpointTiming { clock_offset_ms: 100, playout_latency_ms: 1500 };
        // common start at coord-time 12000 → local issue = 12000+100-1500 = 10600
        assert_eq!(local_issue_time(12_000, &t), 10_600);
    }

    #[test]
    fn drift_correction_is_capped_and_signed() {
        // 100ms late over a 10s window would need 10000 ppm, but we cap at 50
        assert_eq!(drift_correction_ppm(100.0, 10_000.0), 50.0);
        // small drift: 5ms late over 10s = 500 ppm raw -> capped 50
        assert!(drift_correction_ppm(5.0, 10_000.0) > 0.0);
        // early (negative) is pulled the other way
        assert!(drift_correction_ppm(-3.0, 10_000.0) < 0.0);
    }

    #[test]
    fn drift_correction_handles_non_finite() {
        // Non-finite measurements (probe timeout, division underflow,
        // etc.) must not poison the speaker's playback-rate nudge.
        assert_eq!(drift_correction_ppm(f32::NAN, 10_000.0), 0.0);
        assert_eq!(drift_correction_ppm(f32::INFINITY, 10_000.0), 0.0);
        assert_eq!(drift_correction_ppm(f32::NEG_INFINITY, 10_000.0), 0.0);
        assert_eq!(drift_correction_ppm(100.0, f32::NAN), 0.0);
        assert_eq!(drift_correction_ppm(100.0, f32::INFINITY), 0.0);
        assert_eq!(drift_correction_ppm(0.0, 0.0), 0.0);
    }
}
