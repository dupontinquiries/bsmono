// src/transforms.rs
//
// v0.1.342 (redesigned): per-block view state stored INLINE in
// the markdown file, as a fenced ```mermaid-transforms``` block
// immediately following each ```mermaid``` block. Human-readable
// in any text editor AND functional for web2qml.
//
// User direction: "link transforms to the file with inline
// markdowns under the diagrams so that it is human readable in
// a text editor but also functional for web2qml".
//
// Example:
//
//     ```mermaid
//     graph TD
//         A[Start] --> B[Middle]
//         B --> C[End]
//     ```
//
//     ```mermaid-transforms
//     tilt = 35
//     pin A 120.5 88.0
//     pin B 250.0 88.0
//     pin C 380.0 88.0
//     ```
//
// Why inline rather than sidecar:
//   - One file = one document. No orphaned sidecars; no extra
//     git noise.
//   - Diff-friendly: layout changes are right next to the diagram
//     they affect.
//   - Portable: copying the .md file copies the layout too.
//   - Inspectable: a user reading the source sees both the
//     diagram structure AND the saved positions.
//   - Editable: a user can hand-tweak positions in the text
//     editor; web2qml picks them up on next open.
//
// Format details:
//   - Fence: ```mermaid-transforms ... ```
//   - Lines:
//       tilt = <degrees>           # global tilt for THIS block
//       zoom = <factor>            # global zoom for THIS block
//       pin <node_id> <x> <y>      # one pin per line
//   - Order-insensitive. Comments via `#` to end of line.
//   - Unknown lines preserved on round-trip (forward-compat).

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

pub type BlockPins = HashMap<String, (f32, f32)>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockTransforms {
    pub tilt_angle: Option<f32>,
    pub zoom: Option<f32>,
    pub pins: BlockPins,
    /// Lines that didn't match any known directive. Preserved on
    /// round-trip so forward-compat additions don't get stripped
    /// when we rewrite the block.
    pub unrecognized: Vec<String>,
}

impl BlockTransforms {
    pub fn new() -> Self { Self::default() }

    pub fn pin_for(&self, node_id: &str) -> Option<(f32, f32)> {
        self.pins.get(node_id).copied()
    }

    pub fn set_pin(&mut self, node_id: &str, x: f32, y: f32) {
        self.pins.insert(node_id.to_string(), (x, y));
    }

    pub fn unpin(&mut self, node_id: &str) -> bool {
        self.pins.remove(node_id).is_some()
    }

    pub fn clear_pins(&mut self) {
        self.pins.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.tilt_angle.is_none()
            && self.zoom.is_none()
            && self.pins.is_empty()
            && self.unrecognized.is_empty()
    }
}

/// Parse the BODY of a ```mermaid-transforms``` fenced block
/// (everything between the fences). Lenient: malformed lines go
/// to `unrecognized` so a hand-edited file with typos preserves
/// the rest. Unknown directives also go to `unrecognized` for
/// forward-compat.
pub fn parse_block(body: &str) -> BlockTransforms {
    let mut t = BlockTransforms::new();
    for raw_line in body.lines() {
        // Strip trailing `#` comments, then trim.
        let line = match raw_line.split_once('#') {
            Some((before, _)) => before,
            None => raw_line,
        };
        let line = line.trim();
        if line.is_empty() { continue; }
        if try_parse_tilt(line, &mut t)
            || try_parse_zoom(line, &mut t)
            || try_parse_pin(line, &mut t)
        {
            continue;
        }
        t.unrecognized.push(raw_line.to_string());
    }
    t
}

fn try_parse_tilt(line: &str, t: &mut BlockTransforms) -> bool {
    let rest = match line.strip_prefix("tilt") {
        Some(r) => {
            // Must be word-boundary: next char must be space/= or EOL.
            if let Some(c) = r.chars().next() {
                if !c.is_whitespace() && c != '=' { return false; }
            }
            r.trim_start()
        }
        None => return false,
    };
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    if let Ok(v) = rest.parse::<f32>() {
        if v.is_finite() { t.tilt_angle = Some(v); }
        return true;
    }
    false
}

fn try_parse_zoom(line: &str, t: &mut BlockTransforms) -> bool {
    let rest = match line.strip_prefix("zoom") {
        Some(r) => {
            if let Some(c) = r.chars().next() {
                if !c.is_whitespace() && c != '=' { return false; }
            }
            r.trim_start()
        }
        None => return false,
    };
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    if let Ok(v) = rest.parse::<f32>() {
        if v.is_finite() && v > 0.0 { t.zoom = Some(v); }
        return true;
    }
    false
}

fn try_parse_pin(line: &str, t: &mut BlockTransforms) -> bool {
    let rest = match line.strip_prefix("pin") {
        Some(r) => {
            if let Some(c) = r.chars().next() {
                if !c.is_whitespace() { return false; }
            }
            r.trim_start()
        }
        None => return false,
    };
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() != 3 { return false; }
    let id = parts[0];
    if id.is_empty() { return false; }
    let x: f32 = match parts[1].parse() { Ok(v) => v, Err(_) => return false };
    let y: f32 = match parts[2].parse() { Ok(v) => v, Err(_) => return false };
    if !x.is_finite() || !y.is_finite() { return false; }
    t.pins.insert(id.to_string(), (x, y));
    true
}

/// Serialize a BlockTransforms back to the on-disk inline form.
/// Stable output (sorted pin keys) for git-friendly diffs.
/// Returns the BODY content; caller wraps in fence markers.
pub fn serialize_block(t: &BlockTransforms) -> String {
    let mut out = String::with_capacity(128 + t.pins.len() * 32);
    if let Some(angle) = t.tilt_angle {
        out.push_str(&format!("tilt = {:.2}\n", angle));
    }
    if let Some(z) = t.zoom {
        out.push_str(&format!("zoom = {:.3}\n", z));
    }
    let mut keys: Vec<&String> = t.pins.keys().collect();
    keys.sort();
    for k in keys {
        let (x, y) = t.pins[k];
        out.push_str(&format!("pin {} {:.1} {:.1}\n", k, x, y));
    }
    for u in &t.unrecognized {
        out.push_str(u);
        out.push('\n');
    }
    out
}

/// One mermaid block in a source markdown, plus the
/// IMMEDIATELY-following transforms block if present.
/// "Immediately" = only blank lines separate them.
#[derive(Clone, Debug)]
pub struct MermaidBlockLocation {
    /// Byte range of the entire ```mermaid``` fence (including
    /// both opening and closing ``` lines, with trailing newline).
    pub mermaid_range: (usize, usize),
    /// Byte range of the ```mermaid-transforms``` fence (same
    /// inclusive style), or None if no such block follows.
    pub transforms_range: Option<(usize, usize)>,
    /// Parsed transforms (or default if no transforms block).
    pub transforms: BlockTransforms,
}

pub fn locate_mermaid_blocks(source: &str) -> Vec<MermaidBlockLocation> {
    let mut out = Vec::new();
    let bytes = source.as_bytes();
    // split_inclusive keeps the newlines so byte positions are
    // straightforward to track.
    let lines: Vec<&str> = source.split_inclusive('\n').collect();
    let mut line_starts: Vec<usize> = Vec::with_capacity(lines.len() + 1);
    {
        let mut p = 0usize;
        for line in &lines {
            line_starts.push(p);
            p += line.len();
        }
        line_starts.push(bytes.len()); // sentinel
    }

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if fence_language(trimmed).as_deref() == Some("mermaid") {
            let merm_start = line_starts[i];
            // Find closing fence.
            let mut j = i + 1;
            while j < lines.len() && !is_fence_close(lines[j].trim_start()) {
                j += 1;
            }
            // End byte = start of line AFTER the closing fence.
            // If we ran off the end, end is EOF.
            let merm_end = if j < lines.len() {
                line_starts[j + 1]
            } else {
                bytes.len()
            };
            // Look ahead past blank lines for a transforms block.
            let mut k = if j + 1 < lines.len() { j + 1 } else { lines.len() };
            while k < lines.len() && lines[k].trim().is_empty() {
                k += 1;
            }
            let (transforms_range, transforms) = if k < lines.len()
                && fence_language(lines[k].trim_start()).as_deref()
                    == Some("mermaid-transforms")
            {
                let tx_start = line_starts[k];
                let body_start = k + 1;
                let mut tx_body_end = body_start;
                while tx_body_end < lines.len()
                    && !is_fence_close(lines[tx_body_end].trim_start())
                {
                    tx_body_end += 1;
                }
                let tx_end = if tx_body_end < lines.len() {
                    line_starts[tx_body_end + 1]
                } else {
                    bytes.len()
                };
                let body: String = lines[body_start..tx_body_end].concat();
                let parsed = parse_block(&body);
                (Some((tx_start, tx_end)), parsed)
            } else {
                (None, BlockTransforms::new())
            };
            // Advance past whatever we consumed.
            i = match transforms_range {
                Some((_, end)) => line_index_at_or_after(&line_starts, end),
                None => j + 1,
            };
            out.push(MermaidBlockLocation {
                mermaid_range: (merm_start, merm_end),
                transforms_range,
                transforms,
            });
        } else {
            i += 1;
        }
    }
    out
}

fn fence_language(line_trimmed: &str) -> Option<String> {
    if !line_trimmed.starts_with("```") { return None; }
    let rest = line_trimmed.trim_start_matches('`').trim_end();
    let lang = rest.split_whitespace().next().unwrap_or("");
    Some(lang.to_lowercase())
}

fn is_fence_close(line_trimmed: &str) -> bool {
    line_trimmed.starts_with("```")
        && line_trimmed.trim_start_matches('`').trim().is_empty()
}

fn line_index_at_or_after(line_starts: &[usize], byte_offset: usize) -> usize {
    for (i, &s) in line_starts.iter().enumerate() {
        if s >= byte_offset { return i; }
    }
    line_starts.len().saturating_sub(1)
}

/// Update the Nth (0-indexed) mermaid block's transforms in the
/// source. If a transforms block already follows, it's replaced;
/// otherwise a new one is inserted immediately after.
///
/// Returns the new source. If block_idx is out of bounds, returns
/// the source unchanged. If the new transforms are empty AND a
/// transforms block exists, it's removed.
pub fn update_block_transforms(
    source: &str,
    block_idx: usize,
    new_transforms: &BlockTransforms,
) -> String {
    let locations = locate_mermaid_blocks(source);
    let loc = match locations.get(block_idx) {
        Some(l) => l,
        None => return source.to_string(),
    };
    let mut out = String::with_capacity(source.len() + 256);
    if let Some((tx_start, tx_end)) = loc.transforms_range {
        out.push_str(&source[..tx_start]);
        if !new_transforms.is_empty() {
            out.push_str("```mermaid-transforms\n");
            out.push_str(&serialize_block(new_transforms));
            out.push_str("```\n");
        }
        out.push_str(&source[tx_end..]);
    } else if !new_transforms.is_empty() {
        let (_, merm_end) = loc.mermaid_range;
        out.push_str(&source[..merm_end]);
        out.push_str("\n```mermaid-transforms\n");
        out.push_str(&serialize_block(new_transforms));
        out.push_str("```\n");
        out.push_str(&source[merm_end..]);
    } else {
        // No existing block AND empty new transforms — no-op.
        return source.to_string();
    }
    out
}

/// Atomically update a markdown file's inline transforms for one
/// mermaid block. Best-effort; logs failures via eprintln.
pub fn save_inline_update<P: AsRef<Path>>(
    path: P,
    block_idx: usize,
    new_transforms: &BlockTransforms,
) -> std::io::Result<()> {
    let source = std::fs::read_to_string(&path)?;
    let new_source = update_block_transforms(&source, block_idx, new_transforms);
    if new_source == source {
        return Ok(());
    }
    let path_ref = path.as_ref();
    let tmp = path_ref.with_extension("md.transforms-tmp");
    std::fs::write(&tmp, new_source.as_bytes())?;
    std::fs::rename(&tmp, path_ref)?;
    Ok(())
}

/// Load the transforms for the Nth mermaid block in a markdown
/// file. Returns default (empty) on any error.
pub fn load_block_transforms<P: AsRef<Path>>(
    path: P,
    block_idx: usize,
) -> BlockTransforms {
    let source = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return BlockTransforms::new(),
    };
    let locations = locate_mermaid_blocks(&source);
    locations.get(block_idx)
        .map(|loc| loc.transforms.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── BlockTransforms basics ──

    #[test]
    fn default_is_empty() {
        assert!(BlockTransforms::new().is_empty());
    }

    #[test]
    fn set_pin_round_trip() {
        let mut t = BlockTransforms::new();
        t.set_pin("A", 1.0, 2.0);
        assert_eq!(t.pin_for("A"), Some((1.0, 2.0)));
        assert_eq!(t.pin_for("B"), None);
        assert!(!t.is_empty());
    }

    #[test]
    fn unpin_returns_true_when_removed() {
        let mut t = BlockTransforms::new();
        t.set_pin("A", 0.0, 0.0);
        assert!(t.unpin("A"));
        assert!(!t.unpin("A"));
    }

    // ── Block parsing ──

    #[test]
    fn parse_empty_body() {
        assert!(parse_block("").is_empty());
    }

    #[test]
    fn parse_tilt_equals_form() {
        let t = parse_block("tilt = 35\n");
        assert_eq!(t.tilt_angle, Some(35.0));
    }

    #[test]
    fn parse_tilt_space_form() {
        let t = parse_block("tilt 35\n");
        assert_eq!(t.tilt_angle, Some(35.0));
    }

    #[test]
    fn parse_pin_line() {
        let t = parse_block("pin Start 120.5 88.0\n");
        assert_eq!(t.pin_for("Start"), Some((120.5, 88.0)));
    }

    #[test]
    fn parse_multiple_pins() {
        let body = "pin A 10 20\npin B 30 40\npin C 50 60\n";
        let t = parse_block(body);
        assert_eq!(t.pin_for("A"), Some((10.0, 20.0)));
        assert_eq!(t.pin_for("B"), Some((30.0, 40.0)));
        assert_eq!(t.pin_for("C"), Some((50.0, 60.0)));
    }

    #[test]
    fn parse_strips_inline_comments() {
        let body = "tilt = 35   # default tilt\npin A 0 0   # origin\n";
        let t = parse_block(body);
        assert_eq!(t.tilt_angle, Some(35.0));
        assert_eq!(t.pin_for("A"), Some((0.0, 0.0)));
    }

    #[test]
    fn parse_preserves_unknown_lines() {
        let body = "tilt = 35\nfuture_directive arg1 arg2\npin A 0 0\n";
        let t = parse_block(body);
        assert_eq!(t.tilt_angle, Some(35.0));
        assert_eq!(t.pin_for("A"), Some((0.0, 0.0)));
        assert_eq!(t.unrecognized.len(), 1);
        assert!(t.unrecognized[0].contains("future_directive"));
    }

    #[test]
    fn parse_malformed_pin_goes_to_unrecognized() {
        let t = parse_block("pin OnlyId\n");
        assert!(t.pin_for("OnlyId").is_none());
        assert_eq!(t.unrecognized.len(), 1);
    }

    #[test]
    fn parse_rejects_negative_zoom() {
        let t = parse_block("zoom = -1.0\n");
        assert!(t.zoom.is_none());
    }

    #[test]
    fn parse_word_boundary_prevents_false_match() {
        // "tilted = 35" should NOT match the tilt parser.
        let t = parse_block("tilted = 35\n");
        assert!(t.tilt_angle.is_none());
        assert_eq!(t.unrecognized.len(), 1);
    }

    // ── Block serialization ──

    #[test]
    fn serialize_empty_is_empty_string() {
        assert_eq!(serialize_block(&BlockTransforms::new()), "");
    }

    #[test]
    fn serialize_tilt() {
        let mut t = BlockTransforms::new();
        t.tilt_angle = Some(35.0);
        assert_eq!(serialize_block(&t), "tilt = 35.00\n");
    }

    #[test]
    fn serialize_pins_sorted() {
        let mut t = BlockTransforms::new();
        t.set_pin("Z", 9.0, 9.0);
        t.set_pin("A", 1.0, 2.0);
        t.set_pin("M", 5.0, 5.0);
        let out = serialize_block(&t);
        let pos_a = out.find("pin A ").unwrap();
        let pos_m = out.find("pin M ").unwrap();
        let pos_z = out.find("pin Z ").unwrap();
        assert!(pos_a < pos_m);
        assert!(pos_m < pos_z);
    }

    #[test]
    fn serialize_preserves_unknown() {
        let mut t = BlockTransforms::new();
        t.tilt_angle = Some(35.0);
        t.unrecognized.push("future_setting = on".into());
        let out = serialize_block(&t);
        assert!(out.contains("future_setting = on"));
    }

    #[test]
    fn parse_serialize_round_trip() {
        let mut t1 = BlockTransforms::new();
        t1.tilt_angle = Some(35.0);
        t1.zoom = Some(1.5);
        t1.set_pin("A", 10.0, 20.0);
        t1.set_pin("B", 30.0, 40.0);
        t1.unrecognized.push("custom_directive value".into());
        let serialized = serialize_block(&t1);
        let t2 = parse_block(&serialized);
        assert_eq!(t1, t2);
    }

    // ── Mermaid block location ──

    #[test]
    fn locate_no_mermaid_blocks() {
        let md = "# Title\n\nJust prose.\n";
        assert!(locate_mermaid_blocks(md).is_empty());
    }

    #[test]
    fn locate_single_mermaid_no_transforms() {
        let md = "# Title\n\n```mermaid\ngraph TD\n    A --> B\n```\n\nAfter.\n";
        let locs = locate_mermaid_blocks(md);
        assert_eq!(locs.len(), 1);
        assert!(locs[0].transforms_range.is_none());
        assert!(locs[0].transforms.is_empty());
    }

    #[test]
    fn locate_single_mermaid_with_transforms() {
        let md = "# Title\n\n```mermaid\ngraph TD\n    A --> B\n```\n\n```mermaid-transforms\ntilt = 35\npin A 100 50\n```\n\nAfter.\n";
        let locs = locate_mermaid_blocks(md);
        assert_eq!(locs.len(), 1);
        assert!(locs[0].transforms_range.is_some());
        assert_eq!(locs[0].transforms.tilt_angle, Some(35.0));
        assert_eq!(locs[0].transforms.pin_for("A"), Some((100.0, 50.0)));
    }

    #[test]
    fn locate_multiple_mermaid_blocks() {
        let md = "```mermaid\nA --> B\n```\n\n```mermaid-transforms\npin A 0 0\n```\n\n```mermaid\nC --> D\n```\n\n```mermaid-transforms\npin C 50 50\n```\n";
        let locs = locate_mermaid_blocks(md);
        assert_eq!(locs.len(), 2);
        assert_eq!(locs[0].transforms.pin_for("A"), Some((0.0, 0.0)));
        assert_eq!(locs[1].transforms.pin_for("C"), Some((50.0, 50.0)));
    }

    #[test]
    fn locate_transforms_not_attached_to_distant_block() {
        let md = "```mermaid\nA --> B\n```\n\nSome prose here.\n\n```mermaid-transforms\npin A 0 0\n```\n";
        let locs = locate_mermaid_blocks(md);
        assert_eq!(locs.len(), 1);
        assert!(locs[0].transforms_range.is_none(),
            "transforms across prose should NOT attach: {:?}",
            locs[0].transforms_range);
    }

    // ── Update ──

    #[test]
    fn update_inserts_new_transforms_block() {
        let md = "# Doc\n\n```mermaid\nA --> B\n```\n\nAfter.\n";
        let mut t = BlockTransforms::new();
        t.set_pin("A", 100.0, 50.0);
        let new_md = update_block_transforms(md, 0, &t);
        assert!(new_md.contains("```mermaid-transforms\n"),
            "expected new transforms block: {}", new_md);
        assert!(new_md.contains("pin A 100.0 50.0"),
            "expected pin line: {}", new_md);
        assert!(new_md.contains("# Doc"));
        assert!(new_md.contains("After."));
    }

    #[test]
    fn update_replaces_existing_transforms_block() {
        let md = "```mermaid\nA --> B\n```\n\n```mermaid-transforms\npin A 50 50\n```\nAfter.\n";
        let mut t = BlockTransforms::new();
        t.set_pin("A", 999.0, 888.0);
        let new_md = update_block_transforms(md, 0, &t);
        assert!(new_md.contains("pin A 999.0 888.0"));
        assert!(!new_md.contains("pin A 50 50"),
            "old pin line leaked: {}", new_md);
        assert_eq!(new_md.matches("```mermaid-transforms").count(), 1);
    }

    #[test]
    fn update_with_empty_transforms_removes_existing_block() {
        let md = "```mermaid\nA --> B\n```\n\n```mermaid-transforms\npin A 50 50\n```\nAfter.\n";
        let new_md = update_block_transforms(md, 0, &BlockTransforms::new());
        assert!(!new_md.contains("```mermaid-transforms"),
            "empty transforms should remove block: {}", new_md);
        assert!(new_md.contains("After."));
    }

    #[test]
    fn update_does_nothing_for_out_of_bounds_idx() {
        let md = "Just prose.\n";
        let mut t = BlockTransforms::new();
        t.set_pin("A", 0.0, 0.0);
        let new_md = update_block_transforms(md, 0, &t);
        assert_eq!(new_md, md);
    }

    #[test]
    fn update_only_affects_requested_block() {
        let md = "```mermaid\nA --> B\n```\n\n```mermaid-transforms\npin A 1 1\n```\n\n```mermaid\nC --> D\n```\n\n```mermaid-transforms\npin C 2 2\n```\n";
        let mut t = BlockTransforms::new();
        t.set_pin("A", 999.0, 999.0);
        let new_md = update_block_transforms(md, 0, &t);
        assert!(new_md.contains("pin A 999.0 999.0"));
        assert!(!new_md.contains("pin A 1 1"));
        // Second block UNCHANGED.
        assert!(new_md.contains("pin C 2 2"));
    }

    // ── File I/O ──

    #[test]
    fn save_and_load_round_trip() {
        let tmp_dir = std::env::temp_dir();
        let tmp = tmp_dir.join(format!("transforms_inline_test_{}.md",
            std::process::id()));
        let initial = "# Doc\n\n```mermaid\nA --> B\n```\n";
        std::fs::write(&tmp, initial).expect("init write");

        let mut t = BlockTransforms::new();
        t.tilt_angle = Some(45.0);
        t.set_pin("A", 100.0, 50.0);
        save_inline_update(&tmp, 0, &t).expect("save");

        let loaded = load_block_transforms(&tmp, 0);
        assert_eq!(loaded.tilt_angle, Some(45.0));
        assert_eq!(loaded.pin_for("A"), Some((100.0, 50.0)));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let t = load_block_transforms("/nonexistent/path/file.md", 0);
        assert!(t.is_empty());
    }
}
