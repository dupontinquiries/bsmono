//! wake-kit — spend less radio.
//!
//! Two costs the apps pay independently today, both cross-cutting and neither
//! belonging to any one of them.
//!
//! ## The battery cost is wake-ups, not bytes
//!
//! After a transmission a cellular modem stays in a high-power state for
//! **seconds**, waiting for more traffic — the RRC tail. Ten transmissions
//! batched into one window therefore cost roughly a tenth of ten scattered
//! across a minute, even though the bytes are identical.
//!
//! A survey of this monorepo found independent periodic timers at 1s, 2s, 5s,
//! 20s, 60s and 360s across six apps. Every one of those is a wake-up on its own
//! schedule. `snapcore::schedule` already solved this inside the DHT with
//! ID-derived windows; this generalises the idea so anything periodic can join.
//!
//! ## What this kit is not
//!
//! No timers, no sockets, no compressor, no dependencies. It answers two
//! questions — *when should this run* and *is this worth compressing* — and the
//! caller owns the clock and the codec. That is what lets one policy serve an
//! egui app's frame loop, a daemon's tokio timer and a test's fake clock.

/// How urgent a periodic job is, which decides how much it may be delayed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Urgency {
    /// Must run close to its deadline — a call keepalive, a typing indicator.
    /// Never coalesced, because delaying it changes what the user sees.
    Interactive,
    /// Ordinary background upkeep: presence, re-announce, index refresh. This
    /// is the class worth batching and where nearly all the wake-ups are.
    Background,
    /// Runs only when something else has already woken the radio. A log upload
    /// or a metrics flush should never be the reason a modem powers up.
    Opportunistic,
}

/// A periodic job asking when it may run.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub name: String,
    pub period_ms: u64,
    pub urgency: Urgency,
    /// When it last ran, in the caller's clock.
    pub last_ms: u64,
}

impl Job {
    pub fn new(name: &str, period_ms: u64, urgency: Urgency) -> Job {
        Job { name: name.to_string(), period_ms, urgency, last_ms: 0 }
    }

    pub fn due_at(&self) -> u64 { self.last_ms.saturating_add(self.period_ms) }
}

/// The coalescing window length. Everything Background inside one window runs
/// together.
///
/// Thirty seconds. Long enough that a 60-second job and a 20-second job land in
/// the same wake-up regularly, short enough that no background job is delayed
/// past the point a user would notice a stale presence dot.
pub const WINDOW_MS: u64 = 30_000;

/// Spread jobs across the period by a stable per-device offset.
///
/// Without this, every device on a network would wake at the top of the same
/// window and hit the DHT simultaneously — the thundering herd that naive
/// periodic maintenance always produces. The offset is derived from the device
/// id rather than randomised so it survives restarts: a device that re-rolled
/// its offset on every launch would drift back into the herd whenever a network
/// event restarted everybody at once.
pub fn window_offset(device_id: &[u8], window_ms: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in device_id {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h % window_ms.max(1)
}

/// Which jobs should run now, and when to wake next.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Jobs to run in this wake-up, in the order given.
    pub run: Vec<String>,
    /// When the caller should wake again, in its own clock.
    pub next_wake_ms: u64,
    /// True when this wake-up exists only because something Interactive needed
    /// it. Opportunistic work rides along in that case.
    pub radio_already_up: bool,
}

/// Decide what runs now.
///
/// `now_ms` is the caller's clock; `radio_up` says whether the modem is already
/// active for some other reason — a call, a transfer, a user-initiated fetch.
pub fn plan(jobs: &[Job], device_id: &[u8], now_ms: u64, radio_up: bool) -> Plan {
    let offset = window_offset(device_id, WINDOW_MS);
    let mut run = Vec::new();
    let mut next = u64::MAX;

    for j in jobs {
        let due = j.due_at();
        match j.urgency {
            Urgency::Interactive => {
                // Runs on its own deadline. Coalescing it would change what the
                // user sees, which is not a battery decision to make for them.
                if now_ms >= due { run.push(j.name.clone()); }
                else { next = next.min(due); }
            }
            Urgency::Background => {
                // Due, and we are inside this device's slice of the window.
                let window_start = now_ms.saturating_sub(now_ms % WINDOW_MS) + offset;
                let in_window = now_ms >= window_start
                    && now_ms < window_start.saturating_add(WINDOW_MS / 4);
                if now_ms >= due && (in_window || radio_up) {
                    run.push(j.name.clone());
                } else if now_ms >= due {
                    // Overdue but out of window: wake at the next window rather
                    // than immediately, which is the whole saving.
                    next = next.min(next_window(now_ms, offset));
                } else {
                    next = next.min(due.max(next_window(now_ms, offset)));
                }
            }
            Urgency::Opportunistic => {
                // Never the reason to wake. Rides an existing transmission or
                // waits, however overdue it is — a metrics flush that powers up
                // a modem costs more than the metrics are worth.
                if radio_up && now_ms >= due { run.push(j.name.clone()); }
            }
        }
    }

    Plan {
        run,
        next_wake_ms: if next == u64::MAX { now_ms.saturating_add(WINDOW_MS) } else { next },
        radio_already_up: radio_up,
    }
}

fn next_window(now_ms: u64, offset: u64) -> u64 {
    let base = now_ms - now_ms % WINDOW_MS;
    let start = base + offset;
    if start > now_ms { start } else { start + WINDOW_MS }
}

// ── compression ─────────────────────────────────────────────────────────────

/// Whether a payload should be compressed before sending.
///
/// Deliberately a *decision*, not a compressor: the caller already links one
/// (flate2 in voiceterm, gitmind, av1tuner, fao-codec, tomescope) and a kit that
/// picked for them would add a dependency to every consumer to save nobody
/// anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Compress {
    /// Send as-is.
    No,
    /// Worth compressing.
    Yes,
    /// **Must not** be compressed: the payload mixes attacker-influenced data
    /// with a secret, and compression ratio leaks the secret. This is CRIME and
    /// BREACH, and the answer is a refusal rather than a judgement call.
    NeverSecret,
}

/// Smallest payload worth compressing.
///
/// Below roughly a packet's worth, a compressor usually costs more in header
/// bytes and CPU wake time than it saves — and on a radio-bound device the CPU
/// time is battery too.
pub const MIN_COMPRESS_BYTES: usize = 256;

/// Decide whether to compress.
///
/// `mixes_secret_with_attacker_input` is the question that matters and the one
/// callers forget: if a message contains both a token the attacker wants and
/// bytes the attacker chooses, compressed length is an oracle for the token.
/// snapclone's group entries are sealed *then* framed, so the ciphertext is
/// incompressible anyway — which is the right shape and the reason this returns
/// `No` for already-encrypted payloads rather than trying.
pub fn should_compress(
    len: usize,
    already_encrypted: bool,
    mixes_secret_with_attacker_input: bool,
) -> Compress {
    if mixes_secret_with_attacker_input { return Compress::NeverSecret; }
    // Ciphertext is high-entropy: compressing it wastes CPU and usually grows
    // the payload by the compressor's own framing.
    if already_encrypted { return Compress::No; }
    if len < MIN_COMPRESS_BYTES { return Compress::No; }
    Compress::Yes
}

/// Did compression actually help? Callers should send the original when not.
///
/// A compressor that grows a payload is not a rare edge case — it is the normal
/// outcome for anything already compressed, and shipping the larger version
/// spends bandwidth to save none.
pub fn keep_compressed(original: usize, compressed: usize) -> bool {
    compressed < original
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: &[u8] = b"device-aa";

    fn bg(name: &str, period: u64, last: u64) -> Job {
        Job { name: name.into(), period_ms: period, urgency: Urgency::Background, last_ms: last }
    }

    #[test]
    fn interactive_work_is_never_delayed_for_battery() {
        // A keepalive that arrives late changes what the user sees, and that is
        // not a trade a power policy gets to make for them.
        let j = vec![Job { name: "keepalive".into(), period_ms: 1_000,
                           urgency: Urgency::Interactive, last_ms: 0 }];
        let p = plan(&j, DEV, 1_000, false);
        assert_eq!(p.run, vec!["keepalive"]);
    }

    #[test]
    fn opportunistic_work_never_wakes_the_radio_however_overdue() {
        // A metrics flush that powers up a modem costs more than the metrics
        // are worth — and "overdue" must not become a reason.
        let j = vec![Job { name: "metrics".into(), period_ms: 60_000,
                           urgency: Urgency::Opportunistic, last_ms: 0 }];
        assert!(plan(&j, DEV, 10_000_000, false).run.is_empty(),
                "a year overdue is still not a reason to wake");
        assert_eq!(plan(&j, DEV, 10_000_000, true).run, vec!["metrics"],
                   "but it rides an existing transmission");
    }

    #[test]
    fn background_jobs_of_different_periods_land_in_one_wake_up() {
        // The saving. A 20s job and a 60s job on their own timers wake the
        // radio four times a minute between them; coalesced they wake it once.
        let jobs = vec![bg("presence", 20_000, 0), bg("reannounce", 60_000, 0)];
        let offset = window_offset(DEV, WINDOW_MS);
        let now = WINDOW_MS * 4 + offset;      // inside this device's slice
        let p = plan(&jobs, DEV, now, false);
        assert_eq!(p.run.len(), 2, "both ran together: {:?}", p.run);
    }

    #[test]
    fn an_overdue_background_job_waits_for_the_window_rather_than_firing_now() {
        // Firing immediately is what every naive timer does and is exactly the
        // behaviour that scatters wake-ups.
        let jobs = vec![bg("presence", 20_000, 0)];
        let offset = window_offset(DEV, WINDOW_MS);
        // A moment deliberately outside the device's quarter-window slice.
        let now = WINDOW_MS * 4 + offset + WINDOW_MS / 2;
        let p = plan(&jobs, DEV, now, false);
        assert!(p.run.is_empty(), "should have waited");
        assert!(p.next_wake_ms > now, "and scheduled a real wake: {}", p.next_wake_ms);
    }

    #[test]
    fn an_already_up_radio_lets_background_work_ride_along() {
        // If a call or a transfer already has the modem hot, the tail is
        // already being paid — running now is free.
        let jobs = vec![bg("presence", 20_000, 0)];
        let now = WINDOW_MS * 4 + WINDOW_MS / 2;   // out of window
        assert!(plan(&jobs, DEV, now, false).run.is_empty());
        assert_eq!(plan(&jobs, DEV, now, true).run, vec!["presence"]);
    }

    #[test]
    fn devices_do_not_all_wake_at_the_same_instant() {
        // The thundering herd naive periodic maintenance always produces. Two
        // devices must not share a window offset just because they share a
        // schedule.
        let offsets: Vec<u64> = [b"device-aa".as_slice(), b"device-bb", b"device-cc",
                                 b"phone-1", b"phone-2"]
            .iter().map(|d| window_offset(d, WINDOW_MS)).collect();
        let mut uniq = offsets.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert!(uniq.len() >= 4, "offsets collapsed: {offsets:?}");
    }

    #[test]
    fn a_devices_offset_survives_a_restart() {
        // Randomising per launch would drift everyone back into the herd
        // whenever a network event restarted the fleet at once.
        assert_eq!(window_offset(DEV, WINDOW_MS), window_offset(DEV, WINDOW_MS));
    }

    #[test]
    fn an_empty_job_list_still_schedules_a_sane_next_wake() {
        // A caller with nothing to do must not be told to spin.
        let p = plan(&[], DEV, 5_000, false);
        assert!(p.run.is_empty());
        assert!(p.next_wake_ms > 5_000);
    }

    // ── compression ────────────────────────────────────────────────────────

    #[test]
    fn a_payload_mixing_a_secret_with_attacker_input_is_never_compressed() {
        // CRIME and BREACH: compressed length is an oracle for the secret when
        // the attacker can vary bytes beside it. This is a refusal, not a
        // judgement call, which is why it is a distinct variant rather than
        // `No`.
        assert_eq!(should_compress(100_000, false, true), Compress::NeverSecret);
        assert_eq!(should_compress(10, false, true), Compress::NeverSecret,
                   "size is irrelevant to the leak");
    }

    #[test]
    fn ciphertext_is_not_compressed() {
        // High entropy: compressing it burns CPU — which is battery on a
        // radio-bound device — and usually grows the payload by the
        // compressor's own framing. snapclone seals THEN frames, which is the
        // right order and makes this the common case.
        assert_eq!(should_compress(100_000, true, false), Compress::No);
    }

    #[test]
    fn small_payloads_are_not_worth_the_header() {
        assert_eq!(should_compress(MIN_COMPRESS_BYTES - 1, false, false), Compress::No);
        assert_eq!(should_compress(MIN_COMPRESS_BYTES, false, false), Compress::Yes);
    }

    #[test]
    fn a_compressor_that_grew_the_payload_is_discarded() {
        // Not an edge case — it is the normal outcome for anything already
        // compressed, and sending the larger version spends bandwidth to save
        // none.
        assert!(!keep_compressed(1_000, 1_001));
        assert!(!keep_compressed(1_000, 1_000), "equal is not an improvement");
        assert!(keep_compressed(1_000, 400));
    }
}
