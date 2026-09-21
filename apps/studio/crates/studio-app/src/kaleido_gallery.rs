//! kaleido_gallery — choosing a kaleidoscope by looking, not by number.
//!
//! "6 wedges" means nothing until you see it on YOUR frame: the same setting
//! is lace on a tree line and mush on a face. So the options are laid out by
//! their properties and drawn from the selected clip's current frame:
//!
//! * **wedges** across (2, 3, 4, 6, 8, 12) — how many times the picture repeats;
//! * **turn** down (straight, half a wedge) — which part of the picture the
//!   mirror lines cut through, which decides what ends up at the centre;
//! * **centre**, a choice above the grid (middle, left third, right third) —
//!   where the fold is anchored, so it can sit on the subject. Changing it
//!   redraws every thumbnail, because it changes every option.
//!
//! Clicking a thumbnail applies exactly that setting to the selected clips.

use studio_video::kaleido::{self, Kaleido};

/// Thumbnail size: small enough that twelve fold in well under a second.
pub const TW: u32 = 192;
pub const TH: u32 = 108;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Centre { Middle, LeftThird, RightThird }

impl Centre {
    pub fn label(self) -> &'static str {
        match self { Centre::Middle => "middle", Centre::LeftThird => "left third", Centre::RightThird => "right third" }
    }
    fn x(self) -> f32 { match self { Centre::Middle => 0.5, Centre::LeftThird => 1.0 / 3.0, Centre::RightThird => 2.0 / 3.0 } }
}

pub const WEDGES: [u32; 6] = [2, 3, 4, 6, 8, 12];

/// Every option for one centre: (wedges, turned?) -> the setting and a label
/// that names its properties rather than a number.
pub fn options(centre: Centre) -> Vec<(String, Kaleido)> {
    let mut v = Vec::new();
    for turned in [false, true] {
        for &n in &WEDGES {
            // a "turn" of half a wedge moves the mirror lines to where the
            // wedge centres were: different content lands in the middle
            let angle = if turned { 180.0 / n as f32 } else { 0.0 };
            let name = if n == 2 { "mirror".to_string() } else { format!("{n} wedges") };
            let label = format!("{name}{}", if turned { " · turned" } else { "" });
            v.push((label, Kaleido { segments: n, angle, cx: centre.x(), cy: 0.5 }));
        }
    }
    v
}

/// The frame folded by each option, at thumbnail size (RGB).
pub fn thumbnails(px: &[u8], w: u32, h: u32, opts: &[(String, Kaleido)]) -> Vec<Vec<u8>> {
    let small = shrink(px, w, h, TW, TH);
    opts.iter().map(|(_, k)| kaleido::apply(&small, TW, TH, k)).collect()
}

fn shrink(px: &[u8], w: u32, h: u32, tw: u32, th: u32) -> Vec<u8> {
    let mut out = vec![0u8; (tw * th * 3) as usize];
    for y in 0..th { for x in 0..tw {
        let (sx, sy) = ((x * w / tw).min(w - 1), (y * h / th).min(h - 1));
        let (o, s) = (((y * tw + x) * 3) as usize, ((sy * w + sx) * 3) as usize);
        out[o..o + 3].copy_from_slice(&px[s..s + 3]);
    }}
    out
}

/// The gallery window's state: which clip it folds, the centre in use, and
/// the thumbnails drawn for them (rebuilt when either changes).
pub struct Gallery {
    pub frame: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub centre: Centre,
    textures: Vec<egui::TextureHandle>,
    built_for: Option<Centre>,
}

impl Gallery {
    pub fn new(frame: Vec<u8>, w: u32, h: u32) -> Gallery {
        Gallery { frame, w, h, centre: Centre::Middle, textures: Vec::new(), built_for: None }
    }

    /// Draw the gallery; returns the setting clicked, if any.
    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Kaleido> {
        ui.horizontal(|ui| {
            ui.label("centre:");
            for c in [Centre::Middle, Centre::LeftThird, Centre::RightThird] {
                if ui.selectable_label(self.centre == c, c.label()).clicked() { self.centre = c; }
            }
        });
        let opts = options(self.centre);
        if self.built_for != Some(self.centre) {
            self.textures = thumbnails(&self.frame, self.w, self.h, &opts).into_iter().enumerate().map(|(i, px)| {
                let img = egui::ColorImage::from_rgb([TW as usize, TH as usize], &px);
                ui.ctx().load_texture(format!("kaleido-{i}"), img, egui::TextureOptions::LINEAR)
            }).collect();
            self.built_for = Some(self.centre);
        }
        ui.label("across: how many times it repeats · down: straight, or turned half a wedge");
        let mut chosen = None;
        egui::Grid::new("kaleido-grid").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, ((label, k), tex)) in opts.iter().zip(&self.textures).enumerate() {
                ui.vertical(|ui| {
                    let img = egui::Image::new(tex).fit_to_exact_size(egui::vec2(TW as f32 * 0.75, TH as f32 * 0.75));
                    if ui.add(egui::Button::image(img)).on_hover_text(format!("{label} · centre {}", self.centre.label())).clicked() {
                        chosen = Some(*k);
                    }
                    ui.small(label);
                });
                if i % WEDGES.len() == WEDGES.len() - 1 { ui.end_row(); }
            }
        });
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_named_by_their_properties_and_all_differ() {
        let o = options(Centre::Middle);
        assert_eq!(o.len(), 12, "6 wedge counts x straight/turned");
        assert_eq!(o[0].0, "mirror");
        assert_eq!(o[9].0, "6 wedges · turned");
        let mut seen = std::collections::HashSet::new();
        for (_, k) in &o { assert!(seen.insert((k.segments, (k.angle * 100.0) as i32)), "duplicate option {k:?}"); }
        assert!(o.iter().all(|(_, k)| k.cx == 0.5));
        assert!(options(Centre::LeftThird).iter().all(|(_, k)| (k.cx - 1.0 / 3.0).abs() < 1e-6),
                "the centre choice moves every option");
    }

    #[test]
    fn every_option_actually_looks_different_on_a_real_picture() {
        // an asymmetric test picture: the gallery is useless if two choices look the same
        let (w, h) = (320u32, 180u32);
        let mut px = vec![0u8; (w * h * 3) as usize];
        for y in 0..h { for x in 0..w {
            let i = ((y * w + x) * 3) as usize;
            px[i] = (x * 255 / w) as u8; px[i + 1] = (y * 255 / h) as u8; px[i + 2] = ((x * 7 + y * 3) % 256) as u8;
        }}
        let o = options(Centre::Middle);
        let t = thumbnails(&px, w, h, &o);
        assert!(t.iter().all(|th| th.len() == (TW * TH * 3) as usize));
        for i in 0..t.len() { for j in i + 1..t.len() {
            let diff = t[i].iter().zip(&t[j]).filter(|(a, b)| (**a as i32 - **b as i32).abs() > 20).count();
            assert!(diff > (TW * TH) as usize / 20, "{} and {} look nearly the same", o[i].0, o[j].0);
        }}
    }
}
