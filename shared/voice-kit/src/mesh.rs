//! mesh — the fleet voice topology: given a set of trusted devices, decide **where heavy speech
//! compute runs** and **where a recognized command executes**. Pure decisions (the daemon + net
//! layer carry them out).
//!
//! Two principles, both consistent with the fleet's ethos:
//! * **Offload to where the compute is, don't ration where it isn't.** A phone shouldn't run a 1.5GB
//!   whisper model if a trusted desktop can do it faster — spend the mesh's cycles where they're
//!   cheap. (This is "doing more can be more efficient", applied across devices.)
//! * **Local fastpath is the floor.** Offload is a *step-up*, never a requirement: if no trusted
//!   heavy host is reachable, recognition falls back to the on-device fastpath. The mesh degrades
//!   to a single autonomous device — the survival-tool invariant.

/// A device's speech-to-text capability class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SttClass {
    /// No STT (can still capture audio and forward it).
    None,
    /// Fastpath only — the learned recognizer, cheap, no big model.
    Fast,
    /// Full model (whisper-class) — high quality, heavy.
    Heavy,
}

/// A device participating in the voice mesh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceDevice {
    pub id: String,
    /// SSH-trusted (only trusted devices may run our audio or receive our commands).
    pub trusted: bool,
    /// Reachable on the network right now.
    pub reachable: bool,
    /// Has a microphone that participates in capture.
    pub has_mic: bool,
    /// STT capability.
    pub stt: SttClass,
}

impl VoiceDevice {
    fn usable(&self) -> bool {
        self.trusted && self.reachable
    }
}

/// Where to run heavy STT for an utterance captured on `origin`. Preference order:
/// 1. a *trusted, reachable, non-origin* Heavy host — offload to the beefy device;
/// 2. the origin itself, if it is Heavy — local heavy STT, no network;
/// 3. `None` — no heavy host, so the caller uses the local **fastpath** floor.
pub fn stt_host<'a>(origin: &str, devices: &'a [VoiceDevice]) -> Option<&'a VoiceDevice> {
    // best remote heavy host (offload target)
    let remote = devices
        .iter()
        .filter(|d| d.id != origin && d.usable() && d.stt == SttClass::Heavy)
        .min_by(|a, b| a.id.cmp(&b.id)); // stable pick
    if remote.is_some() {
        return remote;
    }
    // else origin if it can do heavy locally
    devices
        .iter()
        .find(|d| d.id == origin && d.stt == SttClass::Heavy)
}

/// Like [`stt_host`], but honour a preference-ordered partner list (the bsettings voice tier list):
/// walk the tiers and take the first trusted, reachable, Heavy partner that isn't the origin. Falls
/// back to [`stt_host`]'s default pick (including local-heavy) when no listed partner qualifies, so
/// an out-of-date tier list never blocks recognition.
pub fn stt_host_ranked<'a>(
    origin: &str,
    devices: &'a [VoiceDevice],
    partners: &[String],
) -> Option<&'a VoiceDevice> {
    for pid in partners {
        if pid == origin {
            continue;
        }
        if let Some(d) = devices
            .iter()
            .find(|d| &d.id == pid && d.usable() && d.stt == SttClass::Heavy)
        {
            return Some(d);
        }
    }
    stt_host(origin, devices)
}

/// Can this utterance be *offloaded* (a trusted heavy host other than the origin exists)? When
/// false the origin recognizes locally (heavy if it can, else fastpath).
pub fn can_offload(origin: &str, devices: &[VoiceDevice]) -> bool {
    devices
        .iter()
        .any(|d| d.id != origin && d.usable() && d.stt == SttClass::Heavy)
}

/// Where a recognized command runs: the device that owns the target intent, if it's trusted +
/// reachable. `None` → the command can't be delivered right now (caller queues or drops).
pub fn command_target<'a>(owner_id: &str, devices: &'a [VoiceDevice]) -> Option<&'a VoiceDevice> {
    devices.iter().find(|d| d.id == owner_id && d.usable())
}

/// The recognition path an origin device will actually take, for display/telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecogPath {
    /// Offloaded to a trusted heavy host.
    Offloaded,
    /// Heavy model on the origin itself.
    LocalHeavy,
    /// The on-device fastpath floor.
    LocalFastpath,
}

pub fn recog_path(origin: &str, devices: &[VoiceDevice]) -> RecogPath {
    if can_offload(origin, devices) {
        RecogPath::Offloaded
    } else if devices.iter().any(|d| d.id == origin && d.stt == SttClass::Heavy) {
        RecogPath::LocalHeavy
    } else {
        RecogPath::LocalFastpath
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, trusted: bool, reachable: bool, stt: SttClass) -> VoiceDevice {
        VoiceDevice { id: id.into(), trusted, reachable, has_mic: true, stt }
    }

    #[test]
    fn offloads_to_trusted_heavy_desktop() {
        let mesh = vec![
            dev("phone", true, true, SttClass::Fast),
            dev("desktop", true, true, SttClass::Heavy),
        ];
        assert_eq!(stt_host("phone", &mesh).unwrap().id, "desktop");
        assert!(can_offload("phone", &mesh));
        assert_eq!(recog_path("phone", &mesh), RecogPath::Offloaded);
    }

    #[test]
    fn falls_back_to_fastpath_when_alone_or_untrusted() {
        // desktop present but NOT trusted → no offload; phone is Fast only → fastpath floor
        let mesh = vec![
            dev("phone", true, true, SttClass::Fast),
            dev("desktop", false, true, SttClass::Heavy),
        ];
        assert!(!can_offload("phone", &mesh));
        assert!(stt_host("phone", &mesh).is_none());
        assert_eq!(recog_path("phone", &mesh), RecogPath::LocalFastpath);
    }

    #[test]
    fn unreachable_heavy_host_is_not_used() {
        let mesh = vec![
            dev("phone", true, true, SttClass::Fast),
            dev("desktop", true, false, SttClass::Heavy), // offline
        ];
        assert!(!can_offload("phone", &mesh));
        assert_eq!(recog_path("phone", &mesh), RecogPath::LocalFastpath);
    }

    #[test]
    fn origin_heavy_runs_locally_when_no_remote() {
        let mesh = vec![dev("desktop", true, true, SttClass::Heavy)];
        assert_eq!(stt_host("desktop", &mesh).unwrap().id, "desktop");
        assert_eq!(recog_path("desktop", &mesh), RecogPath::LocalHeavy);
    }

    #[test]
    fn ranked_picker_honours_tier_list_then_falls_back() {
        let mesh = vec![
            dev("phone", true, true, SttClass::Fast),
            dev("laptop", true, true, SttClass::Heavy),
            dev("desktop", true, true, SttClass::Heavy),
        ];
        // tier list prefers desktop over laptop even though laptop sorts first by id
        let tiers = vec!["desktop".to_string(), "laptop".to_string()];
        assert_eq!(stt_host_ranked("phone", &mesh, &tiers).unwrap().id, "desktop");
        // an unreachable top tier falls through to the next tier
        let mesh2 = vec![
            dev("phone", true, true, SttClass::Fast),
            dev("desktop", true, false, SttClass::Heavy), // offline
            dev("laptop", true, true, SttClass::Heavy),
        ];
        assert_eq!(stt_host_ranked("phone", &mesh2, &tiers).unwrap().id, "laptop");
        // empty/irrelevant tier list → falls back to the default pick
        assert_eq!(stt_host_ranked("phone", &mesh, &[]).unwrap().id, stt_host("phone", &mesh).unwrap().id);
    }

    #[test]
    fn command_routes_only_to_trusted_reachable_owner() {
        let mesh = vec![
            dev("phone", true, true, SttClass::Fast),
            dev("speaker", true, true, SttClass::None),
            dev("tv", true, false, SttClass::None), // offline
        ];
        assert_eq!(command_target("speaker", &mesh).unwrap().id, "speaker");
        assert!(command_target("tv", &mesh).is_none(), "offline owner undeliverable");
        assert!(command_target("unknown", &mesh).is_none());
    }
}
