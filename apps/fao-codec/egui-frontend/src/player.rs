//! Player foundations — the seek-critical logic of fao_app's Player, ported faithfully.
//!
//! An "anchor" is a frame the player can seek to without a previous decoded frame; the
//! seek walk is (anchor -> next -> ... -> target) for T-frame chains. Classification is
//! FORMAT-BASED, by the first payload byte — the GTK app's hard-won lesson transcribed:
//! it originally tested `fhdr.frame_type == 0`, but the worker always stamps frame_type=1
//! for high-res content, so that test produced ZERO anchors on every v8 export and every
//! T-frame failed to find an anchor. The marker table is the truth:
//!
//!   'D' raw DCT I-frame            SELF-CONTAINED (anchor)
//!   'L' lossy-chroma I-frame       SELF-CONTAINED
//!   'P' YUV-split I-frame          SELF-CONTAINED
//!   'K' block-dict K-frame         SELF-CONTAINED (needs trailer dict, no prev frame)
//!   'V' v8 frame                   SELF-CONTAINED (geometry from V8MX trailer + pose)
//!   'Z' zstd-wrapped (always v8)   SELF-CONTAINED (unwraps to V)
//!   'T' translated/predicted       NOT self-contained (needs anchor_ref)
//!   '_' / empty (skip)             NOT self-contained (copies previous)
//!   'B' block fragment             NOT self-contained
use fao_codec_lib as fao_v1;

/// The format-based anchor test. One place, so the frame_type regression can't return.
pub fn is_anchor_marker(m: u8) -> bool { matches!(m, b'D' | b'L' | b'P' | b'K' | b'V' | b'Z') }

pub struct AnchorMap {
    /// Per frame: (prev anchor <= i, next anchor >= i). O(n) build, O(1) lookup.
    pub map: Vec<(Option<u32>, Option<u32>)>,
    /// Anchor frame indices, ascending.
    pub anchors: Vec<u32>,
    /// Per-marker census, for the diagnostic line and the UI breakdown.
    pub by_marker: std::collections::BTreeMap<u8, u32>,
}

impl AnchorMap {
    /// Same summary line the GTK app logs.
    pub fn summary(&self) -> String {
        let n = self.map.len().max(1);
        let breakdown = self.by_marker.iter()
            .map(|(m, c)| format!("{}={}", *m as char, c))
            .collect::<Vec<_>>().join(" ");
        format!("{} anchors out of {} frames ({:.1}%) [{}]",
            self.anchors.len(), self.map.len(),
            self.anchors.len() as f64 / n as f64 * 100.0, breakdown)
    }
}

/// Build the anchor map from per-frame first-payload-byte markers (None = unreadable or
/// empty payload — never an anchor, counted out of the census exactly as the GTK loop
/// `continue`s them). Pure over extracted markers so it tests without a .fao file.
pub fn build_anchor_map(markers: &[Option<u8>]) -> AnchorMap {
    let n = markers.len();
    let mut anchors: Vec<u32> = Vec::with_capacity(n / 30 + 1);
    let mut by_marker = std::collections::BTreeMap::<u8, u32>::new();
    for (i, m) in markers.iter().enumerate() {
        let Some(marker) = m else { continue };
        *by_marker.entry(*marker).or_insert(0) += 1;
        if is_anchor_marker(*marker) { anchors.push(i as u32); }
    }
    // Second pass, transcribed: prev/next per frame via binary search.
    let mut map = vec![(None, None); n];
    for i in 0..n {
        let fi = i as u32;
        let next = match anchors.binary_search(&fi) {
            Ok(pos) => Some(anchors[pos]),
            Err(pos) => anchors.get(pos).copied(),
        };
        let prev = match anchors.binary_search(&fi) {
            Ok(pos) => Some(anchors[pos]),
            Err(pos) if pos > 0 => Some(anchors[pos - 1]),
            _ => None,
        };
        map[i] = (prev, next);
    }
    AnchorMap { map, anchors, by_marker }
}

/// Extract per-frame markers from a .fao byte buffer using the same lib calls the GTK
/// loader chains: header -> index -> per-frame payload first byte.
pub fn markers_from_fao(data: &[u8]) -> Option<Vec<Option<u8>>> {
    let header = fao_v1::read_fao_header(data)?;
    let index = fao_v1::read_fao_index(data, header.frame_count as usize);
    if index.is_empty() { return None; }
    Some(index.iter().map(|e| {
        fao_v1::read_fao_frame(data, e)
            .and_then(|(bytes, _)| bytes.first().copied())
    }).collect())
}

pub struct State {
    pub path: String,
    pub anchor_map: Option<AnchorMap>,
    pub error: Option<String>,
}
impl Default for State {
    fn default() -> Self { State { path: String::new(), anchor_map: None, error: None } }
}
impl State {
    pub fn open(&mut self) {
        self.error = None; self.anchor_map = None;
        match std::fs::read(self.path.trim()) {
            Err(e) => self.error = Some(format!("open failed: {e}")),
            Ok(data) => match markers_from_fao(&data) {
                None => self.error = Some("not a .fao file (header/index parse failed)".into()),
                Some(markers) => self.anchor_map = Some(build_anchor_map(&markers)),
            },
        }
    }
}

pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    let st = &mut app.player;
    ui.heading("▶ Player — anchor analysis");
    ui.label(egui::RichText::new("format-based anchor map: the frames seek can land on without a decoded predecessor").weak().small());
    ui.horizontal(|ui| {
        ui.label(".fao:");
        ui.add(egui::TextEdit::singleline(&mut st.path).desired_width(360.0));
        if ui.button("Open").clicked() { st.open(); }
    });
    if let Some(e) = &st.error { ui.colored_label(ui.visuals().error_fg_color, e); }
    if let Some(am) = &st.anchor_map {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(am.summary()).monospace());
        // anchor timeline strip: one tick per anchor across the frame axis
        let n = am.map.len().max(1);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(720.0), 22.0), egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        for &a in &am.anchors {
            let x = rect.left() + rect.width() * (a as f32 / n as f32);
            p.line_segment([egui::pos2(x, rect.top() + 2.0), egui::pos2(x, rect.bottom() - 2.0)],
                           egui::Stroke::new(1.0, ui.visuals().hyperlink_color));
        }
        ui.label(egui::RichText::new("full decode + A/B comparison are the next Player slices; this lands the seek foundation the GTK app's playback fix depends on").weak().small());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn m(s: &str) -> Vec<Option<u8>> {
        s.chars().map(|c| if c == '.' { None } else { Some(c as u8) }).collect()
    }
    #[test]
    fn the_frame_type_regression_stays_fixed() {
        // Every v8 export: frame_type would have said "no anchors"; markers say ALL anchors.
        let am = build_anchor_map(&m("VVVVVV"));
        assert_eq!(am.anchors.len(), 6);
        assert!(am.map.iter().all(|&(p, n)| p.is_some() && n.is_some()));
    }
    #[test]
    fn mixed_stream_prev_next_table() {
        // D T T K T .  -> anchors at 0 and 3
        let am = build_anchor_map(&m("DTTKT."));
        assert_eq!(am.anchors, vec![0, 3]);
        assert_eq!(am.map[0], (Some(0), Some(0)));
        assert_eq!(am.map[1], (Some(0), Some(3)));
        assert_eq!(am.map[2], (Some(0), Some(3)));
        assert_eq!(am.map[3], (Some(3), Some(3)));
        assert_eq!(am.map[4], (Some(3), None));
        assert_eq!(am.map[5], (Some(3), None));
        // '.' (unreadable/empty) stays out of the census, like the GTK `continue`
        assert!(!am.by_marker.contains_key(&b'.'));
        assert_eq!(am.by_marker[&b'T'], 3);
    }
    #[test]
    fn all_predicted_means_no_anchors_anywhere() {
        let am = build_anchor_map(&m("TTTT"));
        assert!(am.anchors.is_empty());
        assert!(am.map.iter().all(|&e| e == (None, None)));
    }
    #[test]
    fn z_and_k_are_self_contained_b_and_skip_are_not() {
        assert!(is_anchor_marker(b'Z') && is_anchor_marker(b'K'));
        assert!(!is_anchor_marker(b'B') && !is_anchor_marker(b'_') && !is_anchor_marker(b'T'));
    }
    #[test]
    fn summary_matches_the_gtk_log_shape() {
        let s = build_anchor_map(&m("DTTV")).summary();
        assert!(s.contains("2 anchors out of 4 frames (50.0%)"), "{s}");
        assert!(s.contains("D=1") && s.contains("T=2") && s.contains("V=1"), "{s}");
    }
}
