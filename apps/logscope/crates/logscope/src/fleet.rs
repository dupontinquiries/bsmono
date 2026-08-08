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
