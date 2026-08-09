//! fleet.rs — the one entry point that connects the multi-device pieces.
//!
//! Five modules were built for this and none had a caller: `reachability` decides whether a peer can
//! be reached, `deviceplan` decides what a device is good for, `opsched` decides how much work to
//! commit to, `remoteworkspace` attributes a result, `peerrecord` says what a peer publishes. Each
//! was tested in isolation and the composition existed nowhere.
//!
//! **That is the failure `unwired` exists to name**, and it caught this tree at 602 items on the
//! commit that added the fifth of them. Building a sixth would have been the wrong response.
//!
//! ## What composing them exposed
//!
//! The pieces do not quite fit, and only assembling them shows it: `reachability::Peer` carries an
//! id and a path, `deviceplan::Device` carries a triple and a link speed, and **nothing joins them**.
//! A peer is reachable and a device is capable and the fleet needs both facts about one machine.

use crate::{deviceplan, reachability};

/// A machine, with both halves of what is known about it.
///
/// The join `reachability` and `deviceplan` each assumed someone else would make.
#[derive(Clone, PartialEq, Debug)]
pub struct FleetMember {
    pub peer: reachability::Peer,
    pub device: deviceplan::Device,
}

/// Why a member cannot take a task.
#[derive(Clone, PartialEq, Debug)]
pub enum Unavailable {
    /// Cannot be reached. **Checked first**, because capability is irrelevant if packets do not
    /// arrive — and probing capability over a dead link wastes the timeout.
    Unreachable(String),
    /// Reachable, and not suited to this task.
    Unsuited(deviceplan::Unfit),
    /// Reachable and suited, but the path cannot carry this task's data.
    PathTooWeak { path: reachability::Path },
}

impl Unavailable {
    pub fn explain(&self, t: deviceplan::Task) -> String {
        match self {
            Unavailable::Unreachable(why) => why.clone(),
            Unavailable::Unsuited(u) => u.explain(t),
            Unavailable::PathTooWeak { path } => format!(
                "reachable and capable, but {} — and {t:?} moves data continuously, so a path that \
works for a build check does not serve it", path.explain()),
        }
    }
}

/// Can this member take this task, and over what path?
///
/// **The three checks are ordered by what they cost to discover**: reachability is already known,
/// suitability is a comparison, and the path/task interaction needs both. Ordering them the other
/// way would report a capability verdict about a machine that cannot be contacted.
pub fn assign(m: &FleetMember, t: deviceplan::Task)
    -> Result<reachability::Path, Unavailable>
{
    let path = m.peer.assignable().map_err(Unavailable::Unreachable)?;
    deviceplan::fits(&m.device, t).map_err(Unavailable::Unsuited)?;
    // A relayed or hole-punched path is fine for a build check and wrong for anything streaming, and
    // that is a property of the PAIR rather than of either side — which is why neither module could
    // decide it alone.
    if t.bandwidth_bound() && !path.carries_bulk() {
        return Err(Unavailable::PathTooWeak { path });
    }
    Ok(path)
}

/// Everyone who can take the task, and why each of the others cannot.
///
/// Returns the refusals as well, because a fleet that silently shrinks to one machine looks like a
/// fleet of one — and the operator cannot fix what they are not told about.
pub fn candidates<'a>(members: &'a [FleetMember], t: deviceplan::Task)
    -> (Vec<(&'a FleetMember, reachability::Path)>, Vec<String>)
{
    let mut ok = Vec::new();
    let mut refused = Vec::new();
    for m in members {
        match assign(m, t) {
            Ok(p) => ok.push((m, p)),
            Err(e) => refused.push(format!("{}: {}", m.device.name, e.explain(t))),
        }
    }
    (ok, refused)
}

/// What the peer already has loaded.
///
/// Named `ModelPresence` rather than `Residency` because `memplan::Residency` already means a WEIGHT
/// FORMAT — F32, Int8, Q4. Two different ideas under one noun is what the duplicate ratchet exists
/// to catch, and it caught this one on the run that introduced it.
///
/// **`deviceplan` scores hardware and never asks this**, which makes it possible to route work to a
/// machine that is perfectly capable and 66 seconds away from being useful. That figure is measured:
/// Scout Q4_K_M loads in 66.1s on the Ultra, and a peer that has to load it before answering turns a
/// two-second request into a minute-long one.
#[derive(Clone, PartialEq, Debug)]
pub enum ModelPresence {
    /// The model is loaded right now.
    Loaded,
    /// It is on disk and would have to load. Seconds, measured or estimated.
    OnDisk { load_s: f32 },
    /// Not present. It would have to be transferred first — which for a 30 GiB checkpoint over
    /// anything but a LAN is not a delay, it is a different plan.
    Absent { gib: f32 },
}

impl ModelPresence {
    /// Seconds before this peer can start work, over a link of the given speed.
    pub fn time_to_ready_s(&self, link_mib_s: f32) -> f32 {
        match self {
            ModelPresence::Loaded => 0.0,
            ModelPresence::OnDisk { load_s } => *load_s,
            ModelPresence::Absent { gib } => {
                let transfer = gib * 1024.0 / link_mib_s.max(1.0);
                // Plus a load once it lands. Estimated from the transfer size rather than assumed,
                // because a peer that must fetch 30 GiB is not going to load it quickly either.
                transfer + gib * 2.2
            }
        }
    }

    pub fn explain(&self, link_mib_s: f32) -> String {
        let t = self.time_to_ready_s(link_mib_s);
        match self {
            ModelPresence::Loaded => "loaded and ready".into(),
            ModelPresence::OnDisk { .. } => format!(
                "on disk, {t:.0}s to load. Worth it for a batch and not for one request — the load is paid once and amortises over everything sent after it"),
            ModelPresence::Absent { gib } => format!(
                "not present: {gib:.0} GiB to transfer plus a load, ~{t:.0}s. That is not a delay, it is a different plan — pre-stage it or use a peer that has it"),
        }
    }
}

/// Is the wait worth it for work of this size?
///
/// **The comparison is against doing it locally**, not against zero. A 66-second load is absurd for
/// one token and trivial for an hour of batch work, and nothing decides that without knowing how
/// much work is coming.
pub fn worth_the_wait(r: &ModelPresence, link_mib_s: f32, local_est_s: f32, remote_est_s: f32)
    -> Result<f32, String>
{
    let ready = r.time_to_ready_s(link_mib_s);
    let remote_total = ready + remote_est_s;
    if remote_total >= local_est_s {
        return Err(format!(
            "{ready:.0}s to be ready plus {remote_est_s:.0}s of work is {remote_total:.0}s against {local_est_s:.0}s locally — {}", r.explain(link_mib_s)));
    }
    Ok(local_est_s / remote_total)
}

/// Pick one member for a task, given how much work the session can commit to.
///
/// `candidates` returns everyone eligible; something has to choose. The choice is not "fastest
/// machine" — **it is the cheapest machine that clears the bar**, because spending the direct-path
/// 24 GiB peer on a build check leaves nothing for inference when it is asked for next.
///
/// Ties break toward the LOWER capability for the same reason: a fleet that always picks its best
/// machine has one machine.
pub fn choose<'a>(
    members: &'a [FleetMember],
    t: deviceplan::Task,
) -> Result<(&'a FleetMember, reachability::Path), Vec<String>> {
    let (mut ok, refused) = candidates(members, t);
    if ok.is_empty() { return Err(refused) }
    // Cheapest-that-clears: sort by capability ASCENDING and take the first. For a bandwidth-bound
    // task the eligible set is already restricted to direct paths, so this does not undercut it.
    ok.sort_by(|a, b| a.0.device.gpu_gib.partial_cmp(&b.0.device.gpu_gib)
        .unwrap_or(std::cmp::Ordering::Equal));
    Ok(ok.remove(0))
}

/// Everything the fleet can do right now, as one line per task kind.
///
/// **Reports the empty cases too.** "No machine can verify aarch64 natively" is the fact an operator
/// needs before they wonder why a build never gets checked, and a summary that lists only successes
/// hides exactly that.
pub fn capability_summary(members: &[FleetMember]) -> Vec<String> {
    use deviceplan::Task::*;
    [Inference, Draft, Delegate, BuildCheck, NativeRun].iter().map(|t| {
        let (ok, _) = candidates(members, *t);
        if ok.is_empty() {
            format!("{t:?}: NO machine available — work of this kind stays local")
        } else {
            format!("{t:?}: {} machine(s)", ok.len())
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deviceplan::Task;
    use crate::reachability::{Discovery, Path};

    fn member(name: &str, gpu: f32, link: f32, path: Option<Path>) -> FleetMember {
        FleetMember {
            peer: reachability::Peer {
                id: name.into(), how_found: Discovery::Dht, path, record_age_s: 10.0,
            },
            device: deviceplan::Device {
                name: name.into(), target_triple: "aarch64-unknown-linux-gnu".into(),
                gpu_gib: gpu, rtt_s: 0.02, link_mib_s: link, can_execute: true,
            },
        }
    }

    #[test]
    fn a_LOADED_peer_beats_a_faster_one_that_must_load() {
        // The gap deviceplan leaves: hardware capability says nothing about readiness, and 66
        // seconds of load turns a two-second request into a minute.
        let loaded = ModelPresence::Loaded;
        let on_disk = ModelPresence::OnDisk { load_s: 66.1 };
        assert_eq!(loaded.time_to_ready_s(1000.0), 0.0);
        assert!(worth_the_wait(&loaded, 1000.0, 10.0, 2.0).is_ok());
        assert!(worth_the_wait(&on_disk, 1000.0, 10.0, 2.0).is_err(), "68s > 10s locally");
    }

    #[test]
    fn the_SAME_load_is_worth_it_for_a_batch() {
        // Amortisation is the whole question: absurd for one token, trivial for an hour of work.
        let on_disk = ModelPresence::OnDisk { load_s: 66.1 };
        let x = worth_the_wait(&on_disk, 1000.0, 3600.0, 600.0).unwrap();
        assert!(x > 5.0, "got {x}x");
    }

    #[test]
    fn an_ABSENT_model_is_a_DIFFERENT_PLAN_not_a_delay() {
        // 30 GiB over a link is not something to wait through; the message says pre-stage rather
        // than leaving someone to watch a progress bar.
        let absent = ModelPresence::Absent { gib: 30.0 };
        assert!(absent.explain(100.0).contains("different plan"));
        assert!(absent.time_to_ready_s(100.0) > 300.0);
    }

    #[test]
    fn a_FASTER_LINK_shortens_an_absent_model_and_not_a_loaded_one() {
        assert!(ModelPresence::Absent { gib: 30.0 }.time_to_ready_s(1000.0)
              < ModelPresence::Absent { gib: 30.0 }.time_to_ready_s(100.0));
        assert_eq!(ModelPresence::Loaded.time_to_ready_s(1.0), 0.0);
    }

    #[test]
    fn the_refusal_compares_against_LOCAL_rather_than_against_zero() {
        // "68 seconds" is not a reason on its own — it is a reason only next to what doing it here
        // would cost.
        let e = worth_the_wait(&ModelPresence::OnDisk { load_s: 66.1 }, 1000.0, 10.0, 2.0).unwrap_err();
        assert!(e.contains("against 10s locally"));
    }

    #[test]
    fn choose_takes_the_CHEAPEST_that_clears_the_bar_not_the_fastest() {
        // Spending the 24 GiB peer on a build check leaves nothing for inference when it is asked
        // for next, and a fleet that always picks its best machine has one machine.
        let ms = vec![
            member("big", 24.0, 1000.0, Some(Path::Direct)),
            member("small", 4.0, 1000.0, Some(Path::Direct)),
        ];
        let (picked, _) = choose(&ms, Task::Delegate).unwrap();
        assert_eq!(picked.device.name, "small");
    }

    #[test]
    fn choose_returns_the_REFUSALS_when_nothing_qualifies() {
        // An empty Err would say "no" without saying why, and the operator cannot fix that.
        let ms = vec![member("nogpu", 0.0, 1000.0, Some(Path::Direct))];
        let e = choose(&ms, Task::Inference).unwrap_err();
        assert_eq!(e.len(), 1);
        assert!(e[0].contains("nogpu"));
    }

    #[test]
    fn the_summary_reports_the_EMPTY_capabilities_too() {
        // "No machine can verify aarch64 natively" is the fact an operator needs BEFORE wondering
        // why a build never gets checked.
        let ms = vec![member("nogpu", 0.0, 1000.0, Some(Path::Direct))];
        let lines = capability_summary(&ms);
        assert!(lines.iter().any(|l| l.contains("Inference: NO machine")));
        assert!(lines.iter().any(|l| l.contains("BuildCheck: 1 machine")));
    }

    #[test]
    fn REACHABILITY_is_checked_before_capability() {
        // Capability is irrelevant if packets do not arrive, and probing it over a dead link wastes
        // the timeout.
        let m = member("gone", 0.0, 10.0, Some(Path::Unreachable));
        assert!(matches!(assign(&m, Task::Inference), Err(Unavailable::Unreachable(_))));
    }

    #[test]
    fn a_RELAYED_path_serves_a_build_and_not_inference() {
        // A property of the PAIR rather than of either side — which is why neither module could
        // decide it alone, and why the composition was the missing piece.
        let m = member("relayed", 24.0, 1000.0, Some(Path::Relayed));
        assert_eq!(assign(&m, Task::BuildCheck), Ok(Path::Relayed));
        assert!(matches!(assign(&m, Task::Inference), Err(Unavailable::PathTooWeak { .. })));
    }

    #[test]
    fn a_GPU_LESS_but_reachable_machine_is_refused_for_inference_and_kept_for_builds() {
        let m = member("pi", 0.0, 1000.0, Some(Path::Direct));
        assert!(matches!(assign(&m, Task::Inference), Err(Unavailable::Unsuited(_))));
        assert!(assign(&m, Task::BuildCheck).is_ok());
    }

    #[test]
    fn REFUSALS_are_returned_alongside_the_candidates() {
        // A fleet that silently shrinks to one machine looks like a fleet of one, and the operator
        // cannot fix what they are not told about.
        let ms = vec![
            member("good", 24.0, 1000.0, Some(Path::Direct)),
            member("nogpu", 0.0, 1000.0, Some(Path::Direct)),
            member("gone", 24.0, 1000.0, Some(Path::Unreachable)),
        ];
        let (ok, refused) = candidates(&ms, Task::Inference);
        assert_eq!(ok.len(), 1);
        assert_eq!(refused.len(), 2);
        assert!(refused.iter().any(|r| r.contains("nogpu")));
    }

    #[test]
    fn an_UNPROBED_peer_is_refused_even_when_the_device_looks_perfect() {
        // The whole point of keeping discovery and reachability apart survives the composition.
        let m = member("unprobed", 24.0, 1000.0, None);
        assert!(matches!(assign(&m, Task::BuildCheck), Err(Unavailable::Unreachable(_))));
    }

    #[test]
    fn the_PATH_refusal_explains_why_the_same_machine_serves_other_work() {
        let m = member("relayed", 24.0, 1000.0, Some(Path::Relayed));
        let e = assign(&m, Task::Inference).unwrap_err();
        assert!(e.explain(Task::Inference).contains("works for a build check does not serve it"));
    }
}
