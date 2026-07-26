//! lab.rs — the Encode Lab: run the real i-frame encoder on a generated test pattern at
//! the chosen settings; report bytes, bits/pixel and PSNR. `run_lab` is pure over its
//! inputs (deterministic pattern → deterministic bytes), which is what the tests pin.
use crate::App;
use fao_codec_lib::encode_iframe_recon;

pub struct State {
    pub quality: f32,
    pub dead_zone: f32,
    pub chroma_boost: i32,
    pub size: u32, // test frame is size×size
    pub report: Option<Report>,
}

impl Default for State {
    fn default() -> Self {
        State { quality: 0.7, dead_zone: 1.0, chroma_boost: 0, size: 128, report: None }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub bytes: usize,
    pub bpp: f64,
    pub psnr: f64,
}

/// A deterministic RGB test pattern with the features codecs care about: smooth
/// gradients (DC + low-frequency), a hard diagonal edge (ringing bait), and a
/// checkered high-frequency patch (quantizer stress).
pub fn test_pattern(size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut rgb = vec![0u8; n * n * 3];
    for y in 0..n {
        for x in 0..n {
            let i = (y * n + x) * 3;
            let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
            let grad = (fx * 255.0) as u8;
            let edge = if fx + fy > 1.0 { 200 } else { 40 };
            let check = if (x / 4 + y / 4) % 2 == 0 { 230 } else { 25 };
            rgb[i] = grad;
            rgb[i + 1] = edge;
            rgb[i + 2] = if fx > 0.5 && fy > 0.5 { check } else { grad / 2 + 60 };
        }
    }
    rgb
}

/// Encode the pattern with the real encoder; PSNR comes back from the recon variant.
pub fn run_lab(size: u32, quality: f32, dead_zone: f32, chroma_boost: i32) -> Report {
    let n = size as usize;
    let rgb = test_pattern(size);
    let (bytes, _bits, psnr) = encode_iframe_recon(&rgb, n, n, quality, dead_zone, chroma_boost);
    Report {
        bytes: bytes.len(),
        bpp: bytes.len() as f64 * 8.0 / (n * n) as f64,
        psnr,
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Encode Lab");
    ui.label(egui::RichText::new("the real i-frame encoder on a synthetic torture frame").weak().small());
    ui.separator();
    let s = &mut app.lab;
    ui.add(egui::Slider::new(&mut s.quality, 0.05..=1.0).text("quality"));
    ui.add(egui::Slider::new(&mut s.dead_zone, 0.0..=3.0).text("dead zone"));
    ui.add(egui::Slider::new(&mut s.chroma_boost, -2..=4).text("chroma boost"));
    ui.horizontal(|ui| {
        for sz in [64u32, 128, 256] {
            if ui.selectable_label(s.size == sz, format!("{sz}²")).clicked() {
                s.size = sz;
            }
        }
        if ui.button("▶ encode").clicked() {
            s.report = Some(run_lab(s.size, s.quality, s.dead_zone, s.chroma_boost));
        }
    });
    if let Some(r) = &s.report {
        ui.separator();
        ui.monospace(format!("{} bytes   {:.3} bpp   PSNR {:.2} dB", r.bytes, r.bpp, r.psnr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_encoder_runs_and_quality_moves_the_tradeoff() {
        let lo = run_lab(64, 0.15, 1.0, 0);
        let hi = run_lab(64, 0.95, 1.0, 0);
        assert!(lo.bytes > 0 && hi.bytes > 0);
        assert!(hi.bytes >= lo.bytes, "higher quality must not shrink the stream (lo={} hi={})", lo.bytes, hi.bytes);
        assert!(hi.psnr > lo.psnr, "higher quality must raise PSNR (lo={:.2} hi={:.2})", lo.psnr, hi.psnr);
        assert!(hi.psnr > 25.0, "q=0.95 on a 64² pattern should exceed 25 dB (got {:.2})", hi.psnr);
    }

    #[test]
    fn the_lab_is_deterministic() {
        assert_eq!(run_lab(64, 0.5, 1.0, 1), run_lab(64, 0.5, 1.0, 1));
        assert_eq!(test_pattern(32), test_pattern(32));
    }
}
