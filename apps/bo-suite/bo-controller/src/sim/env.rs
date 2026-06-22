//! `SimEnvironment` — the simulated world. What's actually true about
//! devices on the network, BT speakers, the PulseAudio sink table,
//! and audio sources. The model uses this to determine the outcome
//! of each action (succeed, fail, partially-succeed).
//!
//! The split between `ControllerState` and `SimEnvironment` is the
//! distinction between "what the controller believes" and "what is
//! actually the case." Many bugs live in the gap between the two:
//! e.g., the controller thinks a multi-room is active, but the
//! actual PA combine-sink got torn down by an external event.

use std::collections::BTreeMap;

/// The firmware family of a B&O device. Same three-way classification
/// as the real `firmware_family.rs` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareFamily {
    /// Current Mozart firmware. Accepts URI playback via REST.
    Mozart,
    /// Older Mozart firmware on Stage/Level/Balance/Emerge. SetVolume
    /// works via /BeoZone, but URI playback is rejected with "path
    /// not whitelisted" or 404.
    MozartLegacy,
    /// The very-old Network Music Engine (BeolinkConverse). Behind
    /// nginx, different endpoint shape entirely.
    BeolinkConverse,
}

impl FirmwareFamily {
    /// Whether the device will accept arbitrary HTTP URL playback
    /// via REST. Used to decide whether play_uri / shadow stream
    /// should even attempt the device.
    pub fn accepts_uri_playback(self) -> bool {
        matches!(self, FirmwareFamily::Mozart)
    }
    /// Whether REST volume reads work.
    pub fn rest_volume_works(self) -> bool {
        // Both Mozart and MozartLegacy support GetVolume; BeolinkConverse
        // uses a different endpoint shape and we treat it as no for
        // the model's purposes.
        matches!(self, FirmwareFamily::Mozart | FirmwareFamily::MozartLegacy)
    }
    /// Whether the device supports push notifications. (None means
    /// the controller hasn't probed yet.)
    pub fn supports_push_notify(self) -> bool {
        matches!(self, FirmwareFamily::Mozart)
    }
}

/// One simulated device on the network.
#[derive(Debug, Clone)]
pub struct DeviceProfile {
    pub url: String,
    pub product_name: String,
    pub firmware: FirmwareFamily,
    /// If true, the device responds to mDNS / mux discovery. If
    /// false, the controller never sees it (simulates "device
    /// powered off" or "DNS-SD failed").
    pub reachable: bool,
    /// Volume the device REST currently reports. 0..100.
    pub rest_volume: i32,
}

/// One simulated paired Bluetooth speaker.
#[derive(Debug, Clone)]
pub struct SpeakerProfile {
    pub mac: String,
    pub name: String,
    /// Behavior profile for `bluetoothctl connect`.
    pub connect_outcome: ConnectOutcome,
    /// Is this speaker's bluez_sink CURRENTLY visible in `pactl list
    /// short sinks`? Independent of connect_outcome — a speaker may
    /// be connected but its PA sink may not be visible (RACE during
    /// renegotiation, A2DP profile not active yet, etc.).
    pub pa_sink_visible: bool,
    /// The PA sink name when visible. Always
    /// `bluez_sink.<mac_underscored>.a2dp_sink` in practice.
    pub pa_sink_name: String,
}

impl SpeakerProfile {
    /// Build a speaker profile from a MAC + friendly name with the
    /// common-case defaults (connect succeeds, sink visible).
    pub fn happy(mac: &str, name: &str) -> Self {
        let underscored = mac.replace(':', "_");
        SpeakerProfile {
            mac: mac.to_string(),
            name: name.to_string(),
            connect_outcome: ConnectOutcome::Success,
            pa_sink_visible: true,
            pa_sink_name: format!("bluez_sink.{}.a2dp_sink", underscored),
        }
    }
    /// A speaker that fails to connect — like Smokey in the user's
    /// log. The sink isn't visible either.
    pub fn unreliable(mac: &str, name: &str) -> Self {
        let underscored = mac.replace(':', "_");
        SpeakerProfile {
            mac: mac.to_string(),
            name: name.to_string(),
            connect_outcome: ConnectOutcome::Fails,
            pa_sink_visible: false,
            pa_sink_name: format!("bluez_sink.{}.a2dp_sink", underscored),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// `bluetoothctl connect <mac>` succeeds in normal time (≤8s).
    Success,
    /// Connect times out / fails. Speaker doesn't become a usable sink.
    Fails,
    /// Succeeds but then drops within ~30s of usage (simulates poor
    /// signal / range). Used by sink-disappeared tests.
    SuccessThenDrops,
}

/// Audio source format. The model uses this to decide whether the
/// driver needs ffmpeg piping (anything non-WAV does) and to validate
/// playback-routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AudioFormat {
    Wav,
    Flac,
    Opus,
    Mp3,
    M4a,
    Ogg,
}

impl AudioFormat {
    /// Does paplay need ffmpeg as a decoder upstream? Only WAV is
    /// natively understood; everything else needs decoding.
    pub fn needs_ffmpeg_decode(self) -> bool {
        !matches!(self, AudioFormat::Wav)
    }
    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Wav  => "wav",
            AudioFormat::Flac => "flac",
            AudioFormat::Opus => "opus",
            AudioFormat::Mp3  => "mp3",
            AudioFormat::M4a  => "m4a",
            AudioFormat::Ogg  => "ogg",
        }
    }
}

/// The simulated world.
#[derive(Debug, Clone, Default)]
pub struct SimEnvironment {
    pub devices: BTreeMap<String, DeviceProfile>,
    pub speakers: BTreeMap<String, SpeakerProfile>,
    /// Was system BT powered when the simulation started?
    pub initial_bt_powered: bool,
    /// Current system BT power state (mutable across actions).
    pub bt_currently_powered: bool,
    /// The audio sources the user might try to play. Tests typically
    /// pre-populate this and reference by index.
    pub catalog: Vec<TrackProfile>,
}

#[derive(Debug, Clone)]
pub struct TrackProfile {
    pub id: i64,
    pub title: String,
    pub format: AudioFormat,
    /// Approx duration. Used by simulated playback-completion to
    /// determine when `playing_format` should clear.
    pub duration_secs: u32,
}

impl SimEnvironment {
    /// Two B&O devices + four BT speakers. Mirrors the user's actual
    /// setup so traces from the simulator map directly to their
    /// real-world experience.
    pub fn two_devices_four_speakers() -> Self {
        let mut env = SimEnvironment {
            initial_bt_powered: true,
            bt_currently_powered: true,
            ..Default::default()
        };
        env.devices.insert(
            "http://192.168.2.12".into(),
            DeviceProfile {
                url: "http://192.168.2.12".into(),
                product_name: "Beosound Stage".into(),
                firmware: FirmwareFamily::MozartLegacy,
                reachable: true,
                rest_volume: 41,
            },
        );
        env.devices.insert(
            "http://192.168.2.14".into(),
            DeviceProfile {
                url: "http://192.168.2.14".into(),
                product_name: "Beosound Emerge".into(),
                firmware: FirmwareFamily::MozartLegacy,
                reachable: true,
                rest_volume: 21,
            },
        );
        env.speakers.insert(
            "00:09:A7:77:CD:55".into(),
            SpeakerProfile::happy("00:09:A7:77:CD:55", "Emerge01"),
        );
        env.speakers.insert(
            "00:09:A7:77:C2:88".into(),
            SpeakerProfile::happy("00:09:A7:77:C2:88", "Beosound Emerge-36184504"),
        );
        env.speakers.insert(
            "00:09:A7:8A:94:64".into(),
            SpeakerProfile::happy("00:09:A7:8A:94:64", "Beoplay H95"),
        );
        env.speakers.insert(
            "50:1E:2D:5B:E6:EF".into(),
            SpeakerProfile::unreliable("50:1E:2D:5B:E6:EF", "Smokey"),
        );

        env.catalog = vec![
            TrackProfile {
                id: 2060, title: "AI Travis - 002".into(),
                format: AudioFormat::Opus, duration_secs: 180,
            },
            TrackProfile {
                id: 8381, title: "The Kitchen - BMR".into(),
                format: AudioFormat::Opus, duration_secs: 200,
            },
            TrackProfile {
                id: 7947, title: "Classical for Babies".into(),
                format: AudioFormat::Opus, duration_secs: 28800,
            },
            TrackProfile {
                id: 999_001, title: "WAV testfile".into(),
                format: AudioFormat::Wav, duration_secs: 30,
            },
            TrackProfile {
                id: 999_002, title: "FLAC testfile".into(),
                format: AudioFormat::Flac, duration_secs: 200,
            },
            TrackProfile {
                id: 999_003, title: "MP3 testfile".into(),
                format: AudioFormat::Mp3, duration_secs: 240,
            },
        ];
        env
    }

    /// Single-device scenario — useful for simpler tests where the
    /// 4-speaker fanout isn't the focus.
    pub fn one_device_two_speakers() -> Self {
        let mut env = SimEnvironment {
            initial_bt_powered: true,
            bt_currently_powered: true,
            ..Default::default()
        };
        env.devices.insert(
            "http://192.168.2.14".into(),
            DeviceProfile {
                url: "http://192.168.2.14".into(),
                product_name: "Beosound Emerge".into(),
                firmware: FirmwareFamily::Mozart,
                reachable: true,
                rest_volume: 21,
            },
        );
        env.speakers.insert(
            "AA:01".into(),
            SpeakerProfile::happy("AA:01", "Alpha"),
        );
        env.speakers.insert(
            "BB:02".into(),
            SpeakerProfile::happy("BB:02", "Bravo"),
        );
        env.catalog = vec![TrackProfile {
            id: 1,
            title: "test".into(),
            format: AudioFormat::Opus,
            duration_secs: 120,
        }];
        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_capability_predicates() {
        assert!(FirmwareFamily::Mozart.accepts_uri_playback());
        assert!(!FirmwareFamily::MozartLegacy.accepts_uri_playback());
        assert!(!FirmwareFamily::BeolinkConverse.accepts_uri_playback());

        assert!(FirmwareFamily::Mozart.rest_volume_works());
        assert!(FirmwareFamily::MozartLegacy.rest_volume_works());
        assert!(!FirmwareFamily::BeolinkConverse.rest_volume_works());

        assert!(FirmwareFamily::Mozart.supports_push_notify());
        assert!(!FirmwareFamily::MozartLegacy.supports_push_notify());
    }

    #[test]
    fn speaker_happy_builds_well_known_sink_name() {
        let s = SpeakerProfile::happy("00:09:A7:77:CD:55", "Em01");
        assert_eq!(s.pa_sink_name, "bluez_sink.00_09_A7_77_CD_55.a2dp_sink");
        assert!(s.pa_sink_visible);
        assert_eq!(s.connect_outcome, ConnectOutcome::Success);
    }

    #[test]
    fn unreliable_speaker_has_no_sink() {
        let s = SpeakerProfile::unreliable("50:1E:2D:5B:E6:EF", "Smokey");
        assert!(!s.pa_sink_visible);
        assert_eq!(s.connect_outcome, ConnectOutcome::Fails);
    }

    #[test]
    fn format_decode_dispatch() {
        assert!(!AudioFormat::Wav.needs_ffmpeg_decode());
        for fmt in [AudioFormat::Flac, AudioFormat::Opus, AudioFormat::Mp3,
                    AudioFormat::M4a, AudioFormat::Ogg] {
            assert!(fmt.needs_ffmpeg_decode(),
                    "{:?} should need ffmpeg", fmt);
        }
    }

    #[test]
    fn two_dev_four_speaker_scenario_shape() {
        let env = SimEnvironment::two_devices_four_speakers();
        assert_eq!(env.devices.len(), 2);
        assert_eq!(env.speakers.len(), 4);
        // The user's Smokey was the unreliable one.
        let smokey = env.speakers.get("50:1E:2D:5B:E6:EF").unwrap();
        assert_eq!(smokey.connect_outcome, ConnectOutcome::Fails);
        // Both real devices are MozartLegacy → no URI playback.
        for d in env.devices.values() {
            assert_eq!(d.firmware, FirmwareFamily::MozartLegacy);
            assert!(!d.firmware.accepts_uri_playback());
        }
        // Catalog has multiple formats.
        let formats: std::collections::BTreeSet<AudioFormat> =
            env.catalog.iter().map(|t| t.format).collect();
        assert!(formats.contains(&AudioFormat::Opus));
        assert!(formats.contains(&AudioFormat::Wav));
        assert!(formats.contains(&AudioFormat::Flac));
    }
}
