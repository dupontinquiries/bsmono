//! Frame capture via GStreamer pipewiresrc.
//!
//! # postmarketOS / Alpine node discovery
//!
//! On postmarketOS the libcamera node names vary by kernel and camera driver.
//! `FrameCapture::probe_node_name()` runs `pw-dump --no-colors` and searches
//! for nodes with `media.class = Video/Source` and a `node.description` that
//! contains a camera-like keyword. Returns the first matching `node.name`.
//!
//! # Caps negotiation
//!
//! We only constrain format=RGB after videoconvert — never before.
//! pipewiresrc may deliver NV12, YUY2, or YUYV depending on the sensor driver.
//! Inserting `videoconvert ! video/x-raw,format=RGB` lets GStreamer handle
//! the format conversion transparently regardless of what the sensor offers.
//!
//! # target-object
//!
//! `pipewiresrc target-object=<name>` accepts either the `node.name` or the
//! numeric `object.id`. We use `node.name` which is stable across reboots.
//! If the named node doesn't exist, pipewiresrc prints a WARNING and the
//! pipeline stays in PAUSED — we detect this by checking for PLAYING state.

use anyhow::{anyhow, Context, Result};
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use log::{info, warn, debug};

/// A single captured video frame (RGB24).
pub struct Frame {
    pub data:   Vec<u8>,
    pub width:  u32,
    pub height: u32,
}

pub struct FrameCapture {
    pipeline:    gstreamer::Pipeline,
    appsink:     AppSink,
    cached_dims: Option<(u32, u32)>,
}

impl FrameCapture {
    /// Build a capture pipeline from a `pw-dump`-probed node name.
    /// Falls back to the provided default if probing fails.
    pub fn new(pipeline_str: &str) -> Result<Self> {
        let pipeline = gstreamer::parse::launch(pipeline_str)
            .context("Failed to parse GStreamer capture pipeline")?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("Pipeline is not a gstreamer::Pipeline"))?;

        let appsink = pipeline
            .by_name("sink")
            .ok_or_else(|| anyhow!("Pipeline has no element named 'sink'"))?
            .downcast::<AppSink>()
            .map_err(|_| anyhow!("Element 'sink' is not an AppSink"))?;

        // Only constrain format — let width/height and framerate negotiate freely.
        appsink.set_caps(Some(
            &gstreamer::Caps::builder("video/x-raw")
                .field("format", "RGB")
                .build(),
        ));
        appsink.set_sync(false);

        // READY first, then PLAYING so pipewiresrc can negotiate format.
        pipeline
            .set_state(gstreamer::State::Ready)
            .context("Capture pipeline failed to reach READY")?;

        pipeline
            .set_state(gstreamer::State::Playing)
            .context("Capture pipeline failed to reach PLAYING")?;

        // Wait up to 10s for PLAYING — pipewiresrc needs libcamera to open.
        let (state_change, state, _pending) = pipeline
            .state(gstreamer::ClockTime::from_seconds(10));

        if state_change.is_err() || state != gstreamer::State::Playing {
            // Collect any error messages from the bus before giving up
            let bus = pipeline.bus().expect("pipeline has a bus");
            while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
                use gstreamer::MessageView;
                if let MessageView::Error(e) = msg.view() {
                    warn!("GST error: {} — {:?}", e.error(), e.debug());
                }
            }
            let _ = pipeline.set_state(gstreamer::State::Null);
            anyhow::bail!("Capture pipeline did not reach PLAYING within 10s — camera busy or node not found");
        }

        Ok(Self { pipeline, appsink, cached_dims: None })
    }

    /// Pull the next frame with a timeout in milliseconds.
    /// Returns Ok(None) on timeout, Ok(Some(frame)) on success.
    pub fn next_frame_timeout(&mut self, timeout_ms: u64) -> Result<Option<Frame>> {
        let sample = self.appsink
            .try_pull_sample(gstreamer::ClockTime::from_mseconds(timeout_ms));
        let sample = match sample {
            Some(s) => s,
            None    => return Ok(None),
        };

        // Cache dimensions from caps — avoids re-parsing on every frame
        let (w, h) = if let Some(dims) = self.cached_dims {
            dims
        } else {
            let caps = sample.caps()
                .ok_or_else(|| anyhow!("Sample has no caps"))?;
            let s = caps.structure(0)
                .ok_or_else(|| anyhow!("Caps has no structure"))?;
            let w = s.get::<i32>("width")
                .context("No width in caps")? as u32;
            let h = s.get::<i32>("height")
                .context("No height in caps")? as u32;
            info!("Camera negotiated: {}×{} RGB", w, h);
            self.cached_dims = Some((w, h));
            (w, h)
        };

        let buf  = sample.buffer().ok_or_else(|| anyhow!("Sample has no buffer"))?;
        let map  = buf.map_readable().context("Cannot map buffer")?;
        let data = map.as_slice().to_vec();
        drop(map);

        debug!("Frame {}×{} ({} bytes)", w, h, data.len());
        Ok(Some(Frame { data, width: w, height: h }))
    }
}

impl Drop for FrameCapture {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gstreamer::State::Null);
    }
}

// ── Node probing ──────────────────────────────────────────────────────────────

/// Probe `pw-dump` for available libcamera video source nodes.
/// Returns a list of (node.name, node.description) pairs.
pub fn probe_camera_nodes() -> Vec<(String, String)> {
    use std::process::Command;

    // Try pw-dump --no-colors first (Alpine gst-pipewire)
    let output = Command::new("pw-dump")
        .args(["--no-colors"])
        .output()
        .or_else(|_| Command::new("pw-dump").output());

    let json = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => {
            warn!("pw-dump failed — falling back to pw-cli list-objects");
            return probe_nodes_via_pw_cli();
        }
    };

    parse_camera_nodes_from_json(&json)
}

fn parse_camera_nodes_from_json(json: &str) -> Vec<(String, String)> {
    // pw-dump produces a JSON array. We do a fast string scan rather than
    // pulling in a JSON library — we only need two fields per object.
    //
    // Each node block looks like:
    //   { "id": 42, "type": "PipeWire:Interface:Node",
    //     "info": { "props": {
    //       "node.name": "libcamera_input...",
    //       "node.description": "Fairphone Camera (rear)",
    //       "media.class": "Video/Source",
    //       ...
    //     }}}
    //
    // Fast approach: find "Video/Source" markers, then backtrack to get
    // node.name and node.description from the surrounding block.

    let mut results = Vec::new();
    // json is &str (parameter type). The .as_str() method on &str is
    // new in Rust 1.85; use the binding directly to support older rustc.
    let mut search: &str = json;

    while let Some(class_pos) = search.find("\"Video/Source\"") {
        // Look backwards 2000 chars for node.name
        let start = search[..class_pos].len().saturating_sub(2000);
        let window = &json[start..class_pos + 14];

        let name = extract_json_string(window, "node.name");
        let desc = extract_json_string(window, "node.description");

        if let (Some(n), Some(d)) = (name, desc) {
            // Only include libcamera nodes or camera-role nodes
            if n.contains("libcamera") || n.contains("camera") || d.to_lowercase().contains("camera") {
                info!("Found camera node: {} ({})", n, d);
                results.push((n, d));
            }
        }

        // Advance past this match
        if class_pos + 14 < search.len() {
            search = &search[class_pos + 14..];
        } else {
            break;
        }
    }

    if results.is_empty() {
        warn!("pw-dump: no Video/Source nodes found. Check: is libcamera-gstreamer installed?");
        warn!("Install: apk add gst-plugin-libcamera  (or pipewire-libcamera on your distro)");
    }

    results
}

fn extract_json_string(haystack: &str, key: &str) -> Option<String> {
    let search_key = format!("\"{key}\"");
    let pos = haystack.rfind(&search_key)?;
    let after_key = &haystack[pos + search_key.len()..];
    // Skip whitespace and colon
    let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
    if !after_colon.starts_with('"') { return None; }
    let content = &after_colon[1..];
    let end = content.find('"')?;
    Some(content[..end].to_string())
}

fn probe_nodes_via_pw_cli() -> Vec<(String, String)> {
    let output = std::process::Command::new("pw-cli")
        .arg("list-objects")
        .output();
    let text = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return vec![],
    };
    // pw-cli output: look for "node.name" lines near "Video/Source"
    let mut results = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("Video/Source") {
            // Scan ±20 lines for node.name
            let start = i.saturating_sub(20);
            let end   = (i + 20).min(lines.len());
            let mut name = None;
            let mut desc = None;
            for l in &lines[start..end] {
                if let Some(n) = extract_kv(l, "node.name") { name = Some(n); }
                if let Some(d) = extract_kv(l, "node.description") { desc = Some(d); }
            }
            if let Some(n) = name {
                results.push((n, desc.unwrap_or_default()));
            }
        }
    }
    results
}

fn extract_kv(line: &str, key: &str) -> Option<String> {
    // Matches: key = "value" or key = value
    let needle = format!("{key} = ");
    let pos = line.find(&needle)?;
    let val = line[pos + needle.len()..].trim().trim_matches('"').to_string();
    if val.is_empty() { None } else { Some(val) }
}

/// Pick the best rear/front camera node from the probed list.
/// Returns (rear_node, front_node). Falls back to defaults if probing fails.
///
/// This is the legacy two-camera picker used everywhere we only care about
/// "the" rear and front. For devices with multiple back-facing sensors
/// (e.g. FP5 main + ultrawide) use `pick_camera_nodes_multi` to get all
/// of them.
///
/// Currently unused inside the bridge binary (main calls `_multi` directly
/// to enumerate the ultrawide), but kept around as a stable public API
/// for any consumer that only needs the two-camera answer.
#[allow(dead_code)]
pub fn pick_camera_nodes(
    probed: &[(String, String)],
    default_rear:  &str,
    default_front: &str,
) -> (String, String) {
    let multi = pick_camera_nodes_multi(probed, default_rear, default_front);
    (multi.rear, multi.front)
}

/// Result of multi-camera enumeration: one "primary" rear, one front, plus
/// any ADDITIONAL back-facing cameras (typically ultrawide on FP5-style
/// devices). The primary rear is the FIRST `Location: Back` entry reported
/// by libcamera — which on the FP5 is the main wide sensor.
pub struct PickedCameras {
    pub rear:           String,
    pub front:          String,
    /// Extra back-facing cameras beyond `rear`. On FP5 this is the
    /// ultrawide (S5KJN1 on a separate i2c bus). Empty on devices with
    /// only one rear sensor.
    pub extra_rear:     Vec<String>,
}

/// Multi-camera picker. Same algorithm as `pick_camera_nodes` for the
/// primary rear+front, but ALSO returns any additional back-facing
/// cameras the device reports. The bridge uses this to spawn extra
/// virtual nodes (e.g. virtual-cam-rear-ultrawide).
pub fn pick_camera_nodes_multi(
    probed: &[(String, String)],
    default_rear:  &str,
    default_front: &str,
) -> PickedCameras {
    // Try `cam --list` first — gives us explicit Location: Back/Front
    // labels, which is the only reliable way to distinguish multiple
    // back-facing sensors. Keyword matching on PipeWire descriptions
    // can't tell main-rear from ultrawide-rear since both say "rear".
    if let Some(picked) = pick_via_cam_list_multi(probed) {
        info!("Camera nodes selected via `cam --list`: rear={} front={} extra_rear={:?}",
              picked.rear, picked.front, picked.extra_rear);
        return picked;
    }

    // Fallback to single-camera keyword matching for legacy / non-libcamera
    // setups. No way to discover multiple rears without `cam --list`, so
    // extra_rear stays empty here.
    //
    // V32.34+ HARDENING. The old version had three swap/collision risks:
    //   (1) No rear==front guard — if keyword matching + defaults resolved
    //       both roles to the SAME node, the camera "swap" button did
    //       nothing (both views identical) and one facing was unusable.
    //   (2) Defaults weren't validated against the probed set — a compiled
    //       default for a different device produced a dead camera.
    //   (3) First-keyword-match could grab the wrong sensor when several
    //       descriptions were ambiguous.
    // The fixes below resolve roles, then run a consistency pass that (a)
    // prefers probed nodes over compiled defaults, (b) breaks a rear==front
    // tie by exclusion when exactly two cameras exist, and (c) logs loudly
    // when it has to guess so a wrong guess is diagnosable from the field.
    let mut rear  = None;
    let mut front = None;
    for (name, desc) in probed {
        let desc_lower = desc.to_lowercase();
        let name_lower = name.to_lowercase();
        let is_front = desc_lower.contains("front") || name_lower.contains("front")
                    || desc_lower.contains("selfie");
        let is_rear  = desc_lower.contains("rear") || name_lower.contains("rear")
                    || desc_lower.contains("back");

        // Skip a rear keyword hit that would collide with an already-chosen
        // front (and vice versa) — a node can't be both facings. This stops
        // a description carrying BOTH words (rare, but seen on odd firmware)
        // from claiming both roles.
        if is_front && front.is_none() && rear.as_ref() != Some(name) {
            info!("Classified '{name}' as FRONT (description: '{desc}')");
            front = Some(name.clone());
        }
        if is_rear && rear.is_none() && front.as_ref() != Some(name) {
            info!("Classified '{name}' as REAR (description: '{desc}')");
            rear = Some(name.clone());
        }
    }

    // Helper: is this node actually in the probed set?
    let probed_has = |n: &str| probed.iter().any(|(name, _)| name == n);

    // Resolve rear/front, preferring keyword picks, then a probed default,
    // then the compiled default as a last resort.
    let rear = rear.unwrap_or_else(|| {
        if probed_has(default_rear) {
            warn!("No rear camera identified by keyword — using probed default: {default_rear}");
            default_rear.to_string()
        } else {
            warn!("No rear camera identified and compiled default {default_rear} \
                   is not in the probed set — using it anyway (may be a dead node).");
            warn!("Override with PW_NODE_REAR=<node.name> if this is wrong");
            default_rear.to_string()
        }
    });
    let mut front = front.unwrap_or_else(|| {
        if probed_has(default_front) {
            warn!("No front camera identified by keyword — using probed default: {default_front}");
            default_front.to_string()
        } else {
            warn!("No front camera identified and compiled default {default_front} \
                   is not in the probed set — using it anyway (may be a dead node).");
            warn!("Override with PW_NODE_FRONT=<node.name> if this is wrong");
            default_front.to_string()
        }
    });

    // V32.34+ COLLISION BREAK: rear and front must be distinct nodes. If
    // they collided (both keyword-resolved to one camera, or both fell to a
    // shared default), pick the OTHER probed node for front by exclusion.
    // This is the single most common "camera swap does nothing" cause on
    // devices whose descriptions lack facing keywords.
    if rear == front {
        warn!("Rear and front resolved to the SAME node ({rear}) — breaking the tie by exclusion.");
        if let Some((other, _)) = probed.iter().find(|(name, _)| name != &rear) {
            // Two-camera devices: the non-rear node is the front by
            // elimination. More than two: this picks the first other node,
            // which is a guess — logged so it's diagnosable.
            let probed_cam_count = probed.len();
            if probed_cam_count == 2 {
                info!("Two cameras present — assigning the non-rear node as front: {other}");
            } else {
                warn!("{probed_cam_count} cameras present; assigning first non-rear node as front \
                       by guess: {other}. Set PW_NODE_FRONT to override.");
            }
            front = other.clone();
        } else {
            warn!("Only one camera node present — rear and front will share it. \
                   The swap control will have no effect on this device.");
        }
    }

    info!("Camera nodes selected: rear={rear}, front={front} (no extras found)");
    PickedCameras { rear, front, extra_rear: Vec::new() }
}

/// V32.32: parsed entry from `cam --list` output. Public so the
/// `--probe-cameras` CLI mode can dump them for diagnostic purposes
/// before passing through pipeline-binding. On FP5 we've seen
/// substring-fallback misclassify sensors; the only reliable
/// classifier is libcamera's own `Location:` field. Exposing the
/// raw parse lets users see WHICH sensor the bridge thinks is
/// front vs rear, without having to grep through info-level logs.
#[derive(Debug, Clone)]
pub struct CamListEntry {
    /// Sensor model + i2c address, e.g. "imx858 9-001a".
    pub model: String,
    /// Path fragment used to match PipeWire node names, e.g. "camera_29".
    pub path_id: String,
    /// Lowercase location string from `Location:` line, e.g. "back", "front", "external".
    pub location: String,
}

/// Public diagnostic: returns the parsed entries from `cam --list`, OR
/// None if cam is unavailable / output is empty. Does NOT match against
/// PipeWire nodes — just returns the raw libcamera view of what's there.
pub fn parse_cam_list() -> Option<Vec<CamListEntry>> {
    use std::process::Command;
    let output = Command::new("cam").arg("--list").output().ok()?;
    if !output.status.success() { return None; }
    let text = String::from_utf8_lossy(&output.stdout);

    // V32.32: same parsing as pick_via_cam_list_multi but also captures
    // the sensor model string. Format we parse:
    //   1: 'imx858 9-001a' (/base/soc/cci@ac4a000/i2c-bus@1/camera@29)
    //       Location: Back
    let mut current_path: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut entries: Vec<CamListEntry> = Vec::new();
    for line in text.lines() {
        let line_trim = line.trim();
        // Sensor model line: starts with index colon, has model in quotes,
        // then path in parens. Extract both.
        if let Some(model_start) = line_trim.find('\'') {
            if let Some(model_end_rel) = line_trim[model_start+1..].find('\'') {
                let model_end = model_start + 1 + model_end_rel;
                let model = &line_trim[model_start+1 .. model_end];
                // path is after the closing quote
                if let Some(open) = line_trim[model_end..].find('(') {
                    let open_abs = model_end + open;
                    if let Some(close) = line_trim[open_abs..].find(')') {
                        let path = &line_trim[open_abs+1 .. open_abs+close];
                        let normalized = path.replace('@', "_").replace('/', "_");
                        if let Some(idx) = normalized.rfind("camera_") {
                            current_path  = Some(normalized[idx..].to_string());
                            current_model = Some(model.to_string());
                        }
                    }
                }
            }
        }
        if let Some(loc_str) = line_trim.strip_prefix("Location:") {
            if let (Some(p), Some(m)) = (current_path.take(), current_model.take()) {
                entries.push(CamListEntry {
                    model:    m,
                    path_id:  p,
                    location: loc_str.trim().to_lowercase(),
                });
            }
        }
    }
    if entries.is_empty() { None } else { Some(entries) }
}

/// Try to match libcamera's `cam --list` output to the PipeWire node names
/// by sensor model. This is more reliable than keyword matching on
/// descriptions because libcamera explicitly tags Location.
///
/// Multi-rear version: returns ALL back cameras (the first as `rear`, any
/// further ones as `extra_rear`) plus the first `front` camera.
///
/// Example `cam --list` output we parse:
///   Available cameras:
///   1: 'imx858 9-001a'   (/base/soc/cci@ac4a000/i2c-bus@1/camera@29)
///       Location: Back   (main wide)
///   2: 's5kjn1 8-001a'   (/base/soc/cci@ac4a000/i2c-bus@1/camera@1a)
///       Location: Back   (ultrawide — FP5 only)
///   3: 's5kjn1 8-002d'   (/base/soc/cci@ac4b000/i2c-bus@1/camera@10)
///       Location: Front
///
/// We match a sensor's path fragment (e.g. `camera@29`) to the PipeWire
/// node.name (e.g. `libcamera_input._base_soc_..._camera_29`) by suffix.
///
/// On a single-rear device, `extra_rear` is empty. On FP5, `extra_rear`
/// has one entry (the ultrawide on the same i2c bus as main rear).
///
/// V32.32: parse step extracted to `parse_cam_list()` so the
/// `--probe-cameras` CLI mode can dump entries before pipeline binding.
fn pick_via_cam_list_multi(probed: &[(String, String)]) -> Option<PickedCameras> {
    let entries = parse_cam_list()?;

    // Helper: resolve a libcamera path_id (e.g. "camera_29") to its
    // PipeWire node name from the probed list, by SUFFIX match.
    //
    // V32.32: was a substring `contains` match, which had a false-
    // positive risk: path_id "camera_1" would incorrectly match a
    // PipeWire node whose name contained "camera_10". On hardware
    // with sensors at i2c addresses where one's path is a prefix of
    // another's, that produced silently-wrong sensor routing.
    //
    // Now: require the path_id to appear at the END of the PW node
    // name (after the last segment separator). PipeWire libcamera
    // names always end with the camera path fragment, so this is
    // both more correct and robust to future fragment naming changes.
    let resolve_node = |path_id: &str| -> Option<String> {
        let target = path_id.to_lowercase();
        for (name, _desc) in probed {
            let name_lower = name.to_lowercase();
            if name_lower.ends_with(&target) {
                return Some(name.clone());
            }
        }
        // Fall back to substring match if no suffix matches — some
        // PipeWire versions may append additional suffixes we don't
        // know about. Take the LONGEST matching name to avoid the
        // "camera_1 matches camera_10" pitfall.
        let mut best: Option<&String> = None;
        for (name, _desc) in probed {
            if name.to_lowercase().contains(&target) {
                if best.map_or(true, |b| name.len() > b.len()) {
                    best = Some(name);
                }
            }
        }
        best.cloned()
    };

    // Collect all back-located entries in libcamera's order. The FIRST
    // one becomes the primary rear; subsequent entries become extras.
    // libcamera typically orders by i2c address, so the main sensor
    // (which gets initialized first by the kernel) ends up first.
    let mut back_nodes: Vec<String> = Vec::new();
    let mut front_node: Option<String> = None;
    for entry in &entries {
        if let Some(node) = resolve_node(&entry.path_id) {
            if entry.location == "back" && !back_nodes.contains(&node) {
                back_nodes.push(node);
                info!("cam --list: '{}' ({}) → BACK → {}",
                      entry.model, entry.path_id,
                      back_nodes.last().unwrap());
            } else if entry.location == "front" && front_node.is_none() {
                front_node = Some(node.clone());
                info!("cam --list: '{}' ({}) → FRONT → {}",
                      entry.model, entry.path_id, node);
            }
        } else {
            warn!("cam --list entry '{}' ({} {}) had no matching PipeWire node",
                  entry.model, entry.path_id, entry.location);
        }
    }

    let rear = back_nodes.first()?.clone();
    let front = front_node?;
    let extra_rear: Vec<String> = back_nodes.into_iter().skip(1).collect();

    // V32.34+: defensive — if a node somehow resolved to both a back and a
    // front entry (path-fragment collision in resolve_node's fallback
    // substring match, or genuinely ambiguous libcamera output), rear and
    // front would be identical and the swap control would be a no-op. Reject
    // the cam-list result in that case so the caller falls through to the
    // keyword heuristic, which has its own collision-break by exclusion.
    if rear == front {
        warn!("cam --list produced identical rear and front node ({rear}) — \
               rejecting and falling back to keyword heuristic.");
        return None;
    }

    if !extra_rear.is_empty() {
        info!("Detected {} additional back-facing camera(s) beyond primary rear",
              extra_rear.len());
    }

    Some(PickedCameras { rear, front, extra_rear })
}

// ─── Tests ──────────────────────────────────────────────────────────────────
//
// V32.34+: coverage for the rear/front fallback heuristic — specifically the
// swap/collision hardening. These exercise pick_camera_nodes_multi's KEYWORD
// path: in a test environment `cam --list` isn't available, so
// pick_via_cam_list_multi returns None and we fall through to keyword
// matching, which is exactly the path the hardening lives in.
#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> (String, String) { (s.to_string(), String::new()) }
    fn nd(name: &str, desc: &str) -> (String, String) {
        (name.to_string(), desc.to_string())
    }

    #[test]
    fn keyword_match_assigns_distinct_roles() {
        let probed = vec![
            nd("cam_back",  "Fairphone Camera (rear)"),
            nd("cam_front", "Fairphone Camera (front)"),
        ];
        let p = pick_camera_nodes_multi(&probed, "def_rear", "def_front");
        assert_eq!(p.rear,  "cam_back");
        assert_eq!(p.front, "cam_front");
        assert_ne!(p.rear, p.front);
    }

    #[test]
    fn no_keyword_two_cameras_breaks_tie_by_exclusion() {
        // Neither description has a facing word, and both defaults point at
        // the SAME probed node → old code returned rear==front (swap no-op).
        // New code must split them by exclusion.
        let probed = vec![ n("cam_a"), n("cam_b") ];
        let p = pick_camera_nodes_multi(&probed, "cam_a", "cam_a");
        assert_ne!(p.rear, p.front, "rear and front must be distinct");
        assert_eq!(p.rear, "cam_a");
        assert_eq!(p.front, "cam_b", "front should be the other node by exclusion");
    }

    #[test]
    fn collision_with_single_camera_is_tolerated() {
        // Only one camera present: nothing to break the tie with. We accept
        // rear==front (logged) rather than panicking or returning empty.
        let probed = vec![ n("only_cam") ];
        let p = pick_camera_nodes_multi(&probed, "only_cam", "only_cam");
        assert_eq!(p.rear, "only_cam");
        assert_eq!(p.front, "only_cam");
    }

    #[test]
    fn front_keyword_only_falls_back_to_default_rear() {
        // One camera identifiable as front; rear falls to the (probed)
        // default. They must remain distinct.
        let probed = vec![
            nd("cam_selfie", "Selfie Camera"),
            n("cam_other"),
        ];
        let p = pick_camera_nodes_multi(&probed, "cam_other", "def_front");
        assert_eq!(p.front, "cam_selfie");
        assert_eq!(p.rear, "cam_other");
        assert_ne!(p.rear, p.front);
    }

    #[test]
    fn back_keyword_classifies_rear() {
        let probed = vec![
            nd("cam0", "Main Camera back"),
            nd("cam1", "front facing"),
        ];
        let p = pick_camera_nodes_multi(&probed, "x", "y");
        assert_eq!(p.rear, "cam0");
        assert_eq!(p.front, "cam1");
    }
}
