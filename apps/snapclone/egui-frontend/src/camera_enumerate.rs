//! camera_enumerate — the step that made the camera list empty.
//!
//! ## The bug
//!
//! `App.cameras` is `Vec::new()` at construction, and the ONLY thing
//! that assigns it is `HostEvent::CamerasFound`. Nothing in the tree
//! sends that event. So the list is permanently empty, the picker draws
//! nothing (`if !self.cameras.is_empty()`), and no USB webcam can ever
//! appear — on any machine, with any hardware.
//!
//! Everything downstream was already built and tested: `camera_select`
//! picks and handles PIP, `camera_source` builds pipelines,
//! `camera_policy` tracks which devices are healthy. All of it takes a
//! device list as input, and nobody produced one.
//!
//! ## PipeWire first, /dev/video* as the fallback
//!
//! The first version of this read `/dev/video*` only, on the reasoning
//! that a USB webcam is reachable through v4l2 with or without a session
//! running. On the Fairphone that produced the field report in C13_TODO
//! §23 exactly: the sensors are libcamera devices with no v4l2 capture
//! node at all, so `/dev/video*` is the SoC's decoder and encoder nodes
//! — `qcom-iris-decoder`, `msm_*` — listed as cameras, a default of
//! `/dev/video0` that hangs on open, and a real camera nowhere in the
//! list. `Device::id` had said "the PipeWire node name" all along and
//! nothing was asking PipeWire.
//!
//! So this asks first: `pw-dump`, the same probe `camera-bridge` uses,
//! parsed for every `Video/Source` node and handed to
//! `camera_select::device_from_node`, which already reads facing from
//! `api.libcamera.location` and the ISP-sharing group from `device.id`.
//! That is what makes the two proven paths from the Qt era reachable
//! again: the raw libcamera nodes, and the bridge's `virtual-cam-*`
//! nodes when it is running. The bridge's nodes are ranked first because
//! when it is up it holds the sensors, and a second `pipewiresrc` on a
//! held sensor fails to negotiate.
//!
//! When PipeWire publishes nothing — no session, or a desktop with only
//! v4l2 — `/dev/video*` is read as before, and the node name is reported
//! in the form the backend already targets. When PipeWire publishes
//! anything, the v4l2 list is NOT appended: a running session already
//! publishes the v4l2 cameras it can see, and what it does not publish
//! is the decoder nodes that started this.
//!
//! ## Why the odd-numbered nodes are skipped
//!
//! A single UVC webcam registers TWO `/dev/video` nodes: one for
//! capture and one for metadata. Listing both shows every webcam twice,
//! and selecting the metadata node yields a stream that never produces a
//! frame — which looks exactly like a broken camera. The capability
//! check is what separates them.

use crate::camera_select::{Device, Facing};

/// Read the device list from v4l2.
///
/// Returns empty rather than an error when `/dev` has no video nodes:
/// a machine with no camera is an ordinary state, not a failure, and the
/// picker already handles an empty list by drawing nothing.
pub fn enumerate() -> Vec<Device> {
    let pw = pipewire_devices();
    if !pw.is_empty() {
        return pw;
    }
    v4l2_devices()
}

/// Every `Video/Source` node the PipeWire session publishes, bridge
/// outputs first. Empty when there is no session or no `pw-dump`.
pub fn pipewire_devices() -> Vec<Device> {
    use std::process::Command;
    // `--no-colors` is needed on the Alpine build the phone runs (the
    // default output carries escape codes) and rejected by older pw-dump,
    // so a failure of either kind falls through to the other spelling.
    let out = match Command::new("pw-dump").arg("--no-colors").output() {
        Ok(o) if o.status.success() => o,
        _ => match Command::new("pw-dump").output() {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        },
    };
    devices_from_pw_dump(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `pw-dump` output. Pure, so the FP5's dump can be a fixture.
///
/// No JSON library: this crate has none and one field set per node is not
/// a reason to add one. The scan is structural — every `"props": {…}`
/// block is walked to its matching brace, string-aware, and its top-level
/// scalar pairs handed to `device_from_node`, which decides what is a
/// camera. Device, Port and Client objects have `props` too and are
/// rejected there by their `media.class`.
pub fn devices_from_pw_dump(json: &str) -> Vec<Device> {
    let mut out: Vec<Device> = Vec::new();
    let mut from = 0;
    while let Some(i) = json[from..].find("\"props\"") {
        let key_at = from + i;
        from = key_at + 7;
        let Some(open) = json[from..].find('{') else { break };
        // Only whitespace and a colon may sit between the key and the
        // brace, or this is a value that happens to be the string "props".
        if !json[from..from + open].trim().eq(":") { continue; }
        let open = from + open;
        let Some(close) = matching_brace(json, open) else { break };
        let props = scalar_pairs(&json[open + 1..close]);
        let refs: Vec<(&str, &str)> = props.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        if let Some(d) = crate::camera_select::device_from_node(&refs) {
            if !out.iter().any(|e| e.id == d.id) {
                out.push(d);
            }
        }
        from = close;
    }
    // Stable: bridge outputs first, everything else in the order the
    // session listed it, which puts a platform's own cameras before a
    // USB one plugged in later.
    out.sort_by_key(|d| !d.id.starts_with("virtual-cam"));
    out
}

/// Index of the `}` matching the `{` at `open`, skipping strings.
fn matching_brace(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut i = open;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' { i += 1; } else if c == b'"' { in_str = false; }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => { depth -= 1; if depth == 0 { return Some(i); } }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// The `"key": value` pairs at the top level of an object body, values
/// as their text: strings unescaped, numbers and booleans verbatim (so
/// `"device.id": 36` reads as `"36"`, which `device_from_node` parses),
/// nested objects and arrays skipped whole.
fn scalar_pairs(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(q) = body[i..].find('"') {
        let ks = i + q + 1;
        let Some(ke) = string_end(body, ks) else { break };
        let key = unescape(&body[ks..ke]);
        i = ke + 1;
        let Some(colon) = body[i..].find(':') else { break };
        if !body[i..i + colon].trim().is_empty() { continue; }
        i += colon + 1;
        let rest = body[i..].trim_start();
        let off = body.len() - rest.len();
        match rest.as_bytes().first() {
            Some(b'"') => {
                let Some(ve) = string_end(body, off + 1) else { break };
                out.push((key, unescape(&body[off + 1..ve])));
                i = ve + 1;
            }
            Some(b'{') | Some(b'[') => {
                let Some(ce) = matching_brace(body, off) else { break };
                i = ce + 1;
            }
            Some(_) => {
                let end = rest.find([',', '}', ']', '\n']).unwrap_or(rest.len());
                out.push((key, rest[..end].trim().to_string()));
                i = off + end;
            }
            None => break,
        }
    }
    out
}

/// Index of the closing quote of the string whose first content byte is
/// at `start`.
fn string_end(s: &str, start: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' { out.push(c); continue; }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

fn v4l2_devices() -> Vec<Device> {
    let Ok(rd) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut nodes: Vec<(u32, String)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n: u32 = name.strip_prefix("video")?.parse().ok()?;
            Some((n, name))
        })
        .collect();
    // Numerically, not lexically: video10 sorts before video2 as a
    // string, and the resulting picker order would look scrambled on any
    // machine with more than ten nodes.
    nodes.sort_by_key(|(n, _)| *n);

    nodes
        .into_iter()
        .filter(|(n, _)| is_capture_node(*n))
        .map(|(n, node)| {
            let label = read_name(n).unwrap_or_else(|| format!("camera {n}"));
            Device {
                facing: facing_of(&label),
                // No capture group: a USB webcam is independent, which is
                // what `None` means here. Cameras that SHARE an ISP (the
                // FP5's front and rear) get a group from the platform
                // backend, and guessing one here would disable PIP for
                // devices that can do it.
                capture_group: None,
                id: node,
                label,
            }
        })
        .collect()
}

/// Is this a capture node rather than a metadata one?
///
/// A single UVC webcam registers two `/dev/video` nodes. Listing both
/// shows every camera twice, and the metadata node produces no frames —
/// a stream that opens and stays black, which reads as broken hardware.
///
/// Read from sysfs rather than by opening the device: opening a camera
/// to ask what it is can block, and can fail if another app holds it —
/// so a camera already in use would vanish from the list rather than
/// appear as busy.
fn is_capture_node(n: u32) -> bool {
    let path = format!("/sys/class/video4linux/video{n}/device/../video4linux/video{n}/name");
    // The presence of a `name` is not the test; the capability is. But
    // `device_caps` is the authoritative field and is absent on some
    // kernels, so fall back to accepting the node — over-listing is
    // recoverable (a device that fails to open), under-listing is not (a
    // camera that does not exist as far as the app is concerned).
    let caps = std::fs::read_to_string(format!("/sys/class/video4linux/video{n}/device_caps"))
        .or_else(|_| std::fs::read_to_string(path));
    match caps {
        Ok(text) => {
            let t = text.trim();
            // `device_caps` is hex; V4L2_CAP_VIDEO_CAPTURE is 0x00000001.
            match u32::from_str_radix(t.trim_start_matches("0x"), 16) {
                Ok(bits) => bits & 0x0000_0001 != 0,
                // Not hex — this was the name fallback, so accept it.
                Err(_) => true,
            }
        }
        Err(_) => true,
    }
}

/// The human name from sysfs, e.g. "Logitech StreamCam".
fn read_name(n: u32) -> Option<String> {
    let s = std::fs::read_to_string(format!("/sys/class/video4linux/video{n}/name")).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Guess which way a camera points, from its name.
///
/// Delegates to `camera_source::looks_front_facing`, which already owns
/// this rule — a second copy would let the picker and the pipeline
/// disagree about mirroring, and a mirrored rear camera is a bug people
/// notice immediately.
fn facing_of(label: &str) -> Facing {
    if crate::camera_source::looks_front_facing(label) {
        Facing::Front
    } else {
        // Unknown, not Rear. A USB webcam on a desk faces the user but
        // is not a phone's "rear" camera, and claiming Rear would apply
        // rear-camera rotation rules to it.
        Facing::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_select::Facing;

    /// The shape `pw-dump` prints, trimmed to what matters: a Device
    /// object (not a camera), the two libcamera sensors with the FP5's
    /// properties, the bridge's front output, another app's video output
    /// (not a camera either), and a microphone.
    const FP5_DUMP: &str = r#"[
  {
    "id": 31,
    "type": "PipeWire:Interface:Device",
    "version": 3,
    "permissions": [ "r", "w", "x", "m" ],
    "info": {
      "change-mask": [ "props", "params" ],
      "props": {
        "object.serial": 31,
        "device.api": "libcamera",
        "media.class": "Video/Device",
        "device.name": "libcamera_device.platform-ac5a000.camera"
      },
      "params": { "EnumProfile": [], "Profile": [] }
    }
  },
  {
    "id": 36,
    "type": "PipeWire:Interface:Node",
    "version": 3,
    "permissions": [ "r", "w", "x", "m" ],
    "info": {
      "max-input-ports": 0,
      "max-output-ports": 1,
      "change-mask": [ "input-ports", "output-ports", "state", "props", "params" ],
      "state": "suspended",
      "error": null,
      "props": {
        "object.serial": 36,
        "factory.name": "api.libcamera.source",
        "node.name": "libcamera_input.platform-ac5a000.camera-1",
        "node.description": "Fairphone \"IMX858\" (back)",
        "media.class": "Video/Source",
        "api.libcamera.location": "back",
        "device.id": 31,
        "node.pause-on-idle": false,
        "priority.session": 1000
      },
      "params": { "EnumFormat": [ { "mediaType": "video", "format": "NV12" } ] }
    }
  },
  {
    "id": 37,
    "type": "PipeWire:Interface:Node",
    "version": 3,
    "info": {
      "props": {
        "node.name": "libcamera_input.platform-ac5a000.camera-2",
        "node.description": "Fairphone S5KJN1 (front)",
        "media.class": "Video/Source",
        "api.libcamera.location": "front",
        "device.id": 31
      }
    }
  },
  {
    "id": 52,
    "type": "PipeWire:Interface:Node",
    "version": 3,
    "info": {
      "props": {
        "media.class": "Video/Source",
        "node.name": "virtual-cam-front",
        "node.description": "Front Camera (camera-bridge)",
        "application.name": "camera-bridge"
      }
    }
  },
  {
    "id": 60,
    "type": "PipeWire:Interface:Node",
    "info": {
      "props": {
        "media.class": "Stream/Output/Video",
        "node.name": "some-app",
        "node.description": "some app's window"
      }
    }
  },
  {
    "id": 61,
    "type": "PipeWire:Interface:Node",
    "info": {
      "props": {
        "media.class": "Audio/Source",
        "node.name": "mic",
        "node.description": "Internal Microphone"
      }
    }
  }
]"#;

    #[test]
    fn the_fairphone_dump_yields_its_cameras_and_nothing_else() {
        let got = devices_from_pw_dump(FP5_DUMP);
        let ids: Vec<&str> = got.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["virtual-cam-front",
                             "libcamera_input.platform-ac5a000.camera-1",
                             "libcamera_input.platform-ac5a000.camera-2"],
                   "the bridge's node first, then the sensors in session order; no device, stream or mic");
        let rear = &got[1];
        assert_eq!(rear.facing, Facing::Rear, "facing read from api.libcamera.location, not the label");
        assert_eq!(rear.label, "Fairphone \"IMX858\" (back)", "escaped quotes unescaped");
        assert_eq!(rear.capture_group, Some(31), "device.id is a NUMBER in pw-dump and must still be read");
        assert_eq!(got[2].facing, Facing::Front);
        assert_eq!(got[2].capture_group, Some(31), "both sensors share the ISP group");
        assert_eq!(got[0].facing, Facing::Front, "the bridge names the facing in its description");
    }

    #[test]
    fn selection_on_the_fairphone_lands_on_a_real_camera() {
        // The §23 report: the default landed on /dev/video0 and hung. With
        // the session's list, the default is the rear sensor.
        let got = devices_from_pw_dump(FP5_DUMP);
        match crate::camera_select::select(&got, "", false) {
            crate::camera_select::Selection::Single { id, .. } =>
                assert_eq!(id, "libcamera_input.platform-ac5a000.camera-1"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_dump_with_no_video_sources_is_empty_not_an_error() {
        assert!(devices_from_pw_dump("[]").is_empty());
        assert!(devices_from_pw_dump("").is_empty());
        assert!(devices_from_pw_dump("{ \"props\": ").is_empty(), "truncated input");
        assert!(devices_from_pw_dump("[{\"info\":{\"props\":{\"media.class\":\"Audio/Sink\",\"node.name\":\"x\"}}}]").is_empty());
    }

    #[test]
    fn a_string_containing_props_or_braces_does_not_derail_the_scan() {
        let dump = r#"[{"info":{"props":{"node.description":"has \"props\": { inside }","media.class":"Video/Source","node.name":"cam"}}}]"#;
        let got = devices_from_pw_dump(dump);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "has \"props\": { inside }");
    }

    #[test]
    fn a_machine_with_no_cameras_returns_empty_rather_than_failing() {
        // The ordinary state on a build machine, and the picker already
        // handles it by drawing nothing. An error here would turn "no
        // webcam plugged in" into a visible fault.
        let list = enumerate();
        // Cannot assert emptiness — a machine might have a loopback device —
        // so assert the shape instead: whatever comes back is well formed.
        //
        // NOT `starts_with("video")`. That held only for the v4l2 fallback,
        // and `enumerate` prefers PipeWire when a session exists — where an id
        // is a NODE NAME like
        // `v4l2_input.platform-580000000.pcie-pci-0000_04_00.0-usb-0_2_1.0`.
        // `camera_select`'s own doc says so in as many words: "pipewiresrc
        // target-object=<node> — not a /dev/videoN path".
        //
        // So the test asserted the fallback's shape and failed on the machine
        // the app is actually for. Found by running the suite on a real device
        // (fleet-logs, 2026-09-04); every build box here has no PipeWire
        // session, takes the v4l2 path, and passed.
        //
        // The real invariant is what the id is USED for: it goes into a
        // gstreamer property, so it must be non-empty and free of whitespace.
        for d in &list {
            assert!(!d.id.is_empty(), "a device with no id cannot be selected");
            assert!(!d.id.contains(char::is_whitespace),
                    "an id with whitespace breaks `pipewiresrc target-object=`: {:?}", d.id);
            assert!(!d.label.is_empty(), "every device needs a label");
        }
    }

    #[test]
    fn nodes_are_ordered_numerically_not_lexically() {
        // video10 sorts before video2 as a string. On a machine with more
        // than ten nodes the picker order would look scrambled.
        let mut v: Vec<(u32, String)> = vec![
            (10, "video10".into()), (2, "video2".into()), (1, "video1".into())];
        v.sort_by_key(|(n, _)| *n);
        assert_eq!(v.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec![1, 2, 10]);
    }

    #[test]
    fn a_usb_webcam_is_unknown_facing_not_rear() {
        // Claiming Rear would apply a phone's rear-camera rotation rules
        // to a webcam on a desk.
        assert_eq!(facing_of("Logitech StreamCam"), Facing::Unknown);
    }

    #[test]
    fn front_facing_detection_is_not_reimplemented_here() {
        // `camera_source::looks_front_facing` owns the rule. A second
        // copy would let the picker and the pipeline disagree about
        // mirroring, and a mirrored rear camera is noticed instantly.
        for name in ["front camera", "Front Facing Cam"] {
            assert_eq!(
                facing_of(name) == Facing::Front,
                crate::camera_source::looks_front_facing(name),
                "diverged from the shared rule on {name:?}"
            );
        }
    }

    #[test]
    fn an_unreadable_capability_accepts_the_node() {
        // Over-listing is recoverable — the device fails to open and the
        // health tracker records it. Under-listing is not: the camera
        // simply does not exist as far as the app is concerned, which is
        // the bug this module was written to fix.
        assert!(is_capture_node(99_999), "an absent node should not be excluded by a read error");
    }
}
