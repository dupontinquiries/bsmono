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

#[derive(Clone, Debug)]
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

/// One opened .fao with its anchor map, decode state and the texture on screen.
pub struct Deck {
    pub path: String,
    pub file: Option<crate::decode::FaoFile>,
    pub anchor_map: Option<AnchorMap>,
    pub error: Option<String>,
    /// The frame the texture shows, so a scrub only re-decodes on change.
    pub shown: Option<usize>,
    pub gap: Option<String>,
    pub tex: Option<egui::TextureHandle>,
}
impl Default for Deck {
    fn default() -> Self {
        Deck { path: String::new(), file: None, anchor_map: None, error: None, shown: None, gap: None, tex: None }
    }
}
impl Deck {
    pub fn open(&mut self) {
        self.error = None; self.anchor_map = None; self.file = None; self.shown = None; self.gap = None; self.tex = None;
        match std::fs::read(self.path.trim()) {
            Err(e) => self.error = Some(format!("open failed: {e}")),
            Ok(data) => match crate::decode::FaoFile::parse(data) {
                None => self.error = Some("not a .fao file (header/index parse failed)".into()),
                Some(f) => {
                    self.anchor_map = Some(build_anchor_map(&f.markers()));
                    self.file = Some(f);
                }
            },
        }
    }
    /// Decode `idx` into the texture if it is not already the frame on screen. A frame the
    /// decoder cannot show keeps the last texture and names the reason.
    pub fn show(&mut self, ctx: &egui::Context, idx: usize, name: &str) {
        if self.shown == Some(idx) { return; }
        let Some(f) = &self.file else { return };
        match f.decode_rgb(idx) {
            Ok(rgb) => {
                let img = egui::ColorImage::from_rgb([f.width(), f.height()], &rgb);
                match &mut self.tex {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => self.tex = Some(ctx.load_texture(format!("fao-{name}"), img, egui::TextureOptions::LINEAR)),
                }
                self.shown = Some(idx); self.gap = None;
            }
            Err(g) => { self.shown = Some(idx); self.gap = Some(g.to_string()); }
        }
    }
}

pub struct State {
    pub a: Deck,
    pub b: Deck,
    pub frame: usize,
    /// A/B split: the fraction of the width that shows A (drag the divider).
    pub split: f32,
    pub playing: bool,
    pub last_tick: Option<std::time::Instant>,
    /// Compatibility with the anchor-analysis slice: A's fields, by the old names.
    pub path: String,
    pub anchor_map: Option<AnchorMap>,
    pub error: Option<String>,
}
impl Default for State {
    fn default() -> Self {
        State { a: Deck::default(), b: Deck::default(), frame: 0, split: 0.5, playing: false, last_tick: None,
                path: String::new(), anchor_map: None, error: None }
    }
}
impl State {
    /// The anchor-analysis entry point kept for callers and tests: opens `path` as deck A.
    pub fn open(&mut self) {
        self.a.path = self.path.clone();
        self.a.open();
        self.anchor_map = self.a.anchor_map.clone();
        self.error = self.a.error.clone();
        self.frame = 0;
    }
    fn frame_count(&self) -> usize { self.a.file.as_ref().map(|f| f.frame_count()).unwrap_or(0) }
    fn step(&mut self, delta: isize) {
        let n = self.frame_count();
        if n == 0 { return; }
        self.frame = (self.frame as isize + delta).clamp(0, n as isize - 1) as usize;
    }
    /// ⏮ / ⏭ in fao_app: the previous / next self-contained frame from the anchor map.
    fn step_anchor(&mut self, forward: bool) {
        let Some(am) = &self.a.anchor_map else { return };
        let pos = am.anchors.partition_point(|&a| (a as usize) < self.frame);
        let target = if forward {
            am.anchors.get(pos + usize::from(am.anchors.get(pos) == Some(&(self.frame as u32))))
        } else {
            pos.checked_sub(1).and_then(|p| am.anchors.get(p))
        };
        if let Some(&t) = target { self.frame = t as usize; }
    }
}

pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    let st = &mut app.player;
    ui.heading("▶ Player — A/B psychovisual comparison");
    ui.label(egui::RichText::new("decoded frames, scrub, anchor stepping; drag the divider to compare A (left) against B (right)").weak().small());
    for (deck, name) in [(&mut st.a, "A"), (&mut st.b, "B")] {
        ui.horizontal(|ui| {
            ui.label(format!("{name}:"));
            ui.add(egui::TextEdit::singleline(&mut deck.path).id_salt(format!("player-path-{name}")).desired_width(360.0));
            if ui.button(format!("Load {name} (.fao)")).clicked() { deck.open(); }
            if let Some(f) = &deck.file {
                ui.label(egui::RichText::new(format!("{}×{} {} frames {:.2} fps{}{}", f.width(), f.height(), f.frame_count(), f.fps(),
                    if f.has_block_dict() { " +dict" } else { "" },
                    if f.has_v8_webs() { " +v8" } else { "" })).weak().small());
            }
        });
        if let Some(e) = &deck.error { ui.colored_label(ui.visuals().error_fg_color, e); }
    }
    if ui.button("B = A").on_hover_text("Load A into B (sanity check)").clicked() {
        st.b.path = st.a.path.clone(); st.b.open();
    }
    st.path = st.a.path.clone();
    st.anchor_map = st.a.anchor_map.clone();
    st.error = st.a.error.clone();

    let n = st.frame_count();
    if n == 0 { return; }

    // Transport. Play advances at the file's fps (25 if the header has none), wall-clocked.
    if st.playing {
        let fps = st.a.file.as_ref().map(|f| f.fps()).filter(|f| *f > 0.0).unwrap_or(25.0);
        let now = std::time::Instant::now();
        let due = st.last_tick.map(|t| now.duration_since(t).as_secs_f64() >= 1.0 / fps).unwrap_or(true);
        if due { st.last_tick = Some(now); st.frame = (st.frame + 1) % n; }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(4));
    }
    ui.horizontal(|ui| {
        if ui.button("⏮").on_hover_text("Previous I-frame").clicked() { st.step_anchor(false); }
        if ui.button("◀").on_hover_text("Previous frame").clicked() { st.step(-1); }
        if ui.button(if st.playing { "⏸ Pause" } else { "▶ Play" }).clicked() { st.playing = !st.playing; st.last_tick = None; }
        if ui.button("▶").on_hover_text("Next frame").clicked() { st.step(1); }
        if ui.button("⏭").on_hover_text("Next I-frame").clicked() { st.step_anchor(true); }
        let mut f = st.frame;
        if ui.add(egui::Slider::new(&mut f, 0..=n - 1).text("frame")).changed() { st.frame = f; }
        if let Some(am) = &st.a.anchor_map {
            let (prev, _) = am.map.get(st.frame).copied().unwrap_or((None, None));
            let marker = st.a.file.as_ref().and_then(|f| f.payload(st.frame)).and_then(|b| b.first().copied()).map(|m| m as char).unwrap_or('·');
            ui.label(egui::RichText::new(format!("'{marker}'  anchor {}", prev.map(|p| p.to_string()).unwrap_or_else(|| "none".into()))).monospace().small());
        }
    });

    // Decode what is on screen.
    let ctx = ui.ctx().clone();
    let frame = st.frame;
    st.a.show(&ctx, frame, "A");
    if st.b.file.is_some() { st.b.show(&ctx, frame, "B"); }
    for (deck, name) in [(&st.a, "A"), (&st.b, "B")] {
        if let Some(g) = &deck.gap { ui.colored_label(ui.visuals().warn_fg_color, format!("{name}: {g}")); }
    }

    // The picture: A, or A|B split with a draggable divider (fao_app's "A/B" view).
    let Some(ta) = st.a.tex.as_ref() else { return };
    let size = ta.size_vec2();
    let avail = ui.available_size();
    let scale = (avail.x / size.x).min(avail.y / size.y).min(4.0).max(0.05);
    let shown = size * scale;
    let (rect, resp) = ui.allocate_exact_size(shown, egui::Sense::click_and_drag());
    let p = ui.painter_at(rect);
    let uv_full = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    match st.b.tex.as_ref() {
        None => { p.image(ta.id(), rect, uv_full, egui::Color32::WHITE); }
        Some(tb) => {
            if resp.dragged() || resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() { st.split = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0); }
            }
            let s = st.split;
            let left = egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + rect.width() * s, rect.bottom()));
            let right = egui::Rect::from_min_max(egui::pos2(left.right(), rect.top()), rect.max);
            p.image(ta.id(), left, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(s, 1.0)), egui::Color32::WHITE);
            p.image(tb.id(), right, egui::Rect::from_min_max(egui::pos2(s, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            p.line_segment([egui::pos2(left.right(), rect.top()), egui::pos2(left.right(), rect.bottom())],
                           egui::Stroke::new(2.0, ui.visuals().hyperlink_color));
            p.text(egui::pos2(rect.left() + 6.0, rect.top() + 4.0), egui::Align2::LEFT_TOP, "A", egui::FontId::proportional(14.0), egui::Color32::WHITE);
            p.text(egui::pos2(rect.right() - 6.0, rect.top() + 4.0), egui::Align2::RIGHT_TOP, "B", egui::FontId::proportional(14.0), egui::Color32::WHITE);
        }
    }

    // The anchor strip, as before.
    if let Some(am) = &st.a.anchor_map {
        ui.label(egui::RichText::new(am.summary()).monospace().small());
        let n = am.map.len().max(1);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(720.0), 14.0), egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        for &a in &am.anchors {
            let x = rect.left() + rect.width() * (a as f32 / n as f32);
            p.line_segment([egui::pos2(x, rect.top() + 2.0), egui::pos2(x, rect.bottom() - 2.0)], egui::Stroke::new(1.0_f32, ui.visuals().hyperlink_color));
        }
        let x = rect.left() + rect.width() * (st.frame as f32 / n as f32);
        p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(2.0_f32, ui.visuals().warn_fg_color));
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
