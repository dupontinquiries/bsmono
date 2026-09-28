//! probe — what cameras this machine has, as text device-kit can decide on.
//!
//! Two questions, two tools: `pw-dump` for the PipeWire session (where a
//! phone's libcamera cameras live) and `v4l2-ctl --list-devices` for the
//! kernel's capture nodes (a desktop webcam). Either tool missing simply
//! contributes nothing — a machine with no camera is an ordinary state.
//!
//! The pw-dump scan is structural and library-free, the way snapclone's
//! is: every `"props": {…}` object is walked to its matching brace and its
//! top-level scalars read. A node is a camera when `media.class` contains
//! `Video/Source` and it has a `node.name` (what `pipewiresrc
//! target-object` takes). `Stream/Output/Video` is another app's output
//! and never a camera — offering it is how a video call shows itself.

/// One PipeWire camera node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PwCamera {
    /// `node.name`, the handle a pipeline opens.
    pub node: String,
    /// `node.description` or `node.nick`, for a picker.
    pub label: String,
    /// Stated by the platform (`api.libcamera.location` / `device.location`),
    /// or guessed from the label; `None` when neither says.
    pub front: Option<bool>,
}

/// Names that mark a front-facing camera when the platform does not say.
pub const FRONT_MARKERS: [&str; 4] = ["front", "user", "selfie", "facetime"];

pub fn looks_front_facing(name: &str) -> bool {
    let n = name.to_lowercase();
    FRONT_MARKERS.iter().any(|m| n.contains(m))
}

/// Parse `pw-dump` output into camera nodes. Pure.
pub fn cameras_from_pw_dump(json: &str) -> Vec<PwCamera> {
    let mut out: Vec<PwCamera> = Vec::new();
    let mut from = 0;
    while let Some(i) = json[from..].find("\"props\"") {
        let key_at = from + i;
        from = key_at + 7;
        let Some(open) = json[from..].find('{') else { break };
        if !json[from..from + open].trim().eq(":") { continue; }
        let open = from + open;
        let Some(close) = matching_brace(json, open) else { break };
        let props = scalar_pairs(&json[open + 1..close]);
        let get = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        if let (Some(class), Some(node)) = (get("media.class"), get("node.name")) {
            if class.contains("Video/Source") && !node.is_empty() && !out.iter().any(|c| c.node == node) {
                let label = get("node.description").or_else(|| get("node.nick")).unwrap_or(node).to_string();
                let front = match get("api.libcamera.location").or_else(|| get("device.location")).map(str::trim) {
                    Some("Front" | "front" | "0") => Some(true),
                    Some("Back" | "back" | "rear" | "Rear" | "1") => Some(false),
                    Some(_) => None,
                    None => if looks_front_facing(&label) { Some(true) } else { None },
                };
                out.push(PwCamera { node: node.to_string(), label, front });
            }
        }
        from = close;
    }
    out
}

/// The libcamera nodes among them — what `device_kit::source_plan::plan`
/// takes as `pw_nodes`. A phone that has them wants them first; a v4l2
/// node on those kernels is the raw sensor, wrong for a viewfinder.
pub fn libcamera_nodes(cams: &[PwCamera]) -> Vec<String> {
    cams.iter().filter(|c| c.node.contains("libcamera")).map(|c| c.node.clone()).collect()
}

/// Run `pw-dump`. Empty when there is no session or no tool.
pub fn pw_dump() -> String {
    use std::process::Command;
    // `--no-colors` is needed on the Alpine build the phone runs and
    // rejected by older pw-dump, so try both spellings.
    match Command::new("pw-dump").arg("--no-colors").output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => match Command::new("pw-dump").output() {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => String::new(),
        },
    }
}

/// Run `v4l2-ctl --list-devices`. Empty when the tool is absent.
pub fn v4l2_list() -> String {
    let (cmd, args) = device_kit::list_video_cmd();
    match std::process::Command::new(cmd).args(args).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => String::new(),
    }
}

/// Everything the machine has, probed once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    pub pw: Vec<PwCamera>,
    pub v4l2: Vec<device_kit::Device>,
    /// The raw v4l2 listing, which the plan builder takes as text.
    pub v4l2_text: String,
}

impl Probe {
    /// Ask the tools.
    pub fn run() -> Probe {
        let v4l2_text = v4l2_list();
        Probe { pw: cameras_from_pw_dump(&pw_dump()), v4l2: device_kit::parse_v4l2_list(&v4l2_text), v4l2_text }
    }

    /// Is there anything to open?
    pub fn any(&self) -> bool {
        !self.pw.is_empty() || self.v4l2.iter().any(|d| d.kind == device_kit::DeviceKind::VideoCapture)
    }

    /// A one-line inventory for a status line.
    pub fn summary(&self) -> String {
        let v = self.v4l2.iter().filter(|d| d.kind == device_kit::DeviceKind::VideoCapture).count();
        match (self.pw.len(), v) {
            (0, 0) => "no camera found".into(),
            (p, 0) => format!("{p} PipeWire camera(s)"),
            (0, v) => format!("{v} v4l2 camera(s)"),
            (p, v) => format!("{p} PipeWire + {v} v4l2 camera(s)"),
        }
    }

    /// The ordered plan, mirrored for a front camera when asked.
    pub fn plan(&self, override_source: Option<&str>, mirror: bool) -> Vec<device_kit::source_plan::SourceCandidate> {
        let flip = if mirror { Some("horizontal-flip") } else { None };
        device_kit::source_plan::plan(&libcamera_nodes(&self.pw), &self.v4l2_text, override_source, flip)
    }
}

fn matching_brace(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let (mut depth, mut in_str, mut i) = (0i32, false, open);
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

/// The `"key": value` pairs at the top level of an object body, values as
/// text; nested objects and arrays skipped whole.
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
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) { out.push(ch); }
                }
                Some(o) => { out.push('\\'); out.push(o); }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = r#"[
  { "id": 40, "type": "PipeWire:Interface:Node", "info": { "props": {
      "media.class": "Video/Source", "node.name": "libcamera__SIMPLE_(platform_1a40000.csiphy)",
      "node.description": "Rear camera", "api.libcamera.location": "Back", "device.id": 36 } } },
  { "id": 41, "type": "PipeWire:Interface:Node", "info": { "props": {
      "media.class": "Video/Source", "node.name": "libcamera__SIMPLE_(platform_1a41000.csiphy)",
      "node.nick": "Front camera", "api.libcamera.location": "0", "device.id": 36 } } },
  { "id": 42, "type": "PipeWire:Interface:Node", "info": { "props": {
      "media.class": "Stream/Output/Video", "node.name": "firefox-call", "node.description": "Firefox" } } },
  { "id": 43, "type": "PipeWire:Interface:Node", "info": { "props": {
      "media.class": "Video/Source", "node.name": "v4l2_input.pci-0000_00_14.0-usb-0_2_1.0",
      "node.description": "Integrated_Webcam_HD: Integrate \"quoted\"" } } },
  { "id": 44, "type": "PipeWire:Interface:Device", "info": { "props": { "media.class": "Video/Device", "device.name": "x" } } }
]"#;

    #[test]
    fn video_sources_are_cameras_and_streams_and_devices_are_not() {
        let cams = cameras_from_pw_dump(DUMP);
        let nodes: Vec<&str> = cams.iter().map(|c| c.node.as_str()).collect();
        assert_eq!(nodes, vec![
            "libcamera__SIMPLE_(platform_1a40000.csiphy)",
            "libcamera__SIMPLE_(platform_1a41000.csiphy)",
            "v4l2_input.pci-0000_00_14.0-usb-0_2_1.0",
        ]);
        assert_eq!(cams[0].front, Some(false), "Back is stated");
        assert_eq!(cams[1].front, Some(true), "0 means front on libcamera");
        assert_eq!(cams[1].label, "Front camera", "nick when there is no description");
        assert_eq!(cams[2].front, None, "a webcam's label says nothing about facing");
        assert!(cams[2].label.contains("\"quoted\""), "escapes are unescaped: {}", cams[2].label);
    }

    #[test]
    fn only_libcamera_nodes_go_first_in_the_plan() {
        let cams = cameras_from_pw_dump(DUMP);
        let lib = libcamera_nodes(&cams);
        assert_eq!(lib.len(), 2);
        let p = Probe { pw: cams, v4l2: Vec::new(), v4l2_text: String::new() };
        let plan = p.plan(None, true);
        assert_eq!(plan.len(), 2, "{plan:?}");
        assert!(plan[0].pipeline.contains("horizontal-flip"), "mirrored for a front preview");
        assert!(p.any());
        assert_eq!(p.summary(), "3 PipeWire camera(s)");
    }

    #[test]
    fn a_desktop_with_only_v4l2_still_has_a_plan() {
        let v4l2 = "Integrated Webcam (usb-0000:00:14.0-5):\n\t/dev/video0\n\t/dev/video1\n\nDummy video device (0x0000) (platform:v4l2loopback-000):\n\t/dev/video10\n";
        let p = Probe { pw: Vec::new(), v4l2: device_kit::parse_v4l2_list(v4l2), v4l2_text: v4l2.into() };
        assert!(p.any());
        assert_eq!(p.summary(), "1 v4l2 camera(s)", "the loopback is not a camera");
        let plan = p.plan(None, false);
        assert_eq!(plan.len(), 1, "{plan:?}");
        assert!(plan[0].pipeline.contains("/dev/video0"));
    }

    #[test]
    fn nothing_found_is_said_plainly() {
        let p = Probe::default();
        assert!(!p.any());
        assert_eq!(p.summary(), "no camera found");
        assert!(p.plan(None, false).is_empty(), "no fabricated candidate");
    }
}
