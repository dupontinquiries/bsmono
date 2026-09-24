//! lab.rs — the Encode Lab: run the real i-frame encoder on a generated test pattern at
//! the chosen settings; report bytes, bits/pixel and PSNR. `run_lab` is pure over its
//! inputs (deterministic pattern → deterministic bytes), which is what the tests pin.
use crate::App;
use fao_codec_lib::encode_iframe_recon;

/// What gets encoded: the synthetic pattern, or an image the user loaded.
#[derive(Clone, PartialEq)]
pub struct Source {
    pub name: String,
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}
impl Source {
    pub fn pattern(size: u32) -> Self {
        Source { name: format!("test pattern {size}²"), w: size as usize, h: size as usize, rgb: test_pattern(size) }
    }
    /// fao_app's "Image" button: any PNG/JPEG, converted to packed RGB8. Odd dimensions are
    /// cropped to even ones — the encoder's chroma planes are half-size in both axes.
    pub fn load(path: &str) -> Result<Self, String> {
        let img = image::open(path.trim()).map_err(|e| format!("{e}"))?.to_rgb8();
        let (w, h) = ((img.width() as usize) & !1, (img.height() as usize) & !1);
        if w < 8 || h < 8 { return Err(format!("image is {}×{}; the encoder needs at least 8×8", img.width(), img.height())); }
        let mut rgb = Vec::with_capacity(w * h * 3);
        for y in 0..h { let row = &img.as_raw()[y * img.width() as usize * 3..]; rgb.extend_from_slice(&row[..w * 3]); }
        let name = std::path::Path::new(path.trim()).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
        Ok(Source { name, w, h, rgb })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View { Source, Decoded, Error }

pub struct State {
    pub quality: f32,
    pub dead_zone: f32,
    pub chroma_boost: i32,
    pub size: u32, // test frame is size×size
    pub source: Source,
    pub image_path: String,
    pub load_error: Option<String>,
    pub report: Option<Report>,
    pub outcome: Option<Outcome>,
    pub view: View,
    pub tex: Option<(View, egui::TextureHandle)>,
    /// The codec to encode with, by codename. See `codec_catalog`.
    pub codec: &'static str,
}
impl Default for State {
    fn default() -> Self {
        State { quality: 0.7, dead_zone: 1.0, chroma_boost: 0, size: 128, source: Source::pattern(128),
                image_path: String::new(), load_error: None, report: None, outcome: None, view: View::Decoded, tex: None,
                codec: "Keystone" }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Compressed size. `encode_iframe_recon` returns (reconstruction, total_bits, psnr);
    /// the first slice reported `reconstruction.len()` here, which is w·h·3 for every
    /// quality — 24.000 bpp on every run, and a test that only asked `hi >= lo` let it
    /// through. bits/8 is the stream.
    pub bytes: usize,
    pub bpp: f64,
    pub psnr: f64,
}

/// The pixels an encode produces, for the Source / Decoded / Error views.
#[derive(Clone, PartialEq)]
pub struct Outcome {
    pub w: usize,
    pub h: usize,
    pub recon: Vec<u8>,
    /// |source − decoded| per channel, ×4 and clamped, so a 2-level error is visible.
    pub error: Vec<u8>,
}

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

/// Encode `src` at the settings; the report and the pictures.
pub fn run_source(src: &Source, quality: f32, dead_zone: f32, chroma_boost: i32) -> (Report, Outcome) {
    let (w, h) = (src.w, src.h);
    let (recon, bits, psnr) = encode_iframe_recon(&src.rgb, w, h, quality, dead_zone, chroma_boost);
    let bytes = (bits as usize).div_ceil(8);
    let error: Vec<u8> = src.rgb.iter().zip(&recon)
        .map(|(a, b)| ((*a as i32 - *b as i32).unsigned_abs() * 4).min(255) as u8).collect();
    (Report { bytes, bpp: bits as f64 / (w * h) as f64, psnr }, Outcome { w, h, recon, error })
}

/// The first slice's entry point, kept: the test pattern at `size`.
pub fn run_lab(size: u32, quality: f32, dead_zone: f32, chroma_boost: i32) -> Report {
    run_source(&Source::pattern(size), quality, dead_zone, chroma_boost).0
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Encode Lab");
    ui.label(egui::RichText::new("the real i-frame encoder on a synthetic torture frame, or on an image you load").weak().small());
    ui.separator();
    let s = &mut app.lab;
    ui.add(egui::Slider::new(&mut s.quality, 0.05..=1.0).text("quality"));
    ui.add(egui::Slider::new(&mut s.dead_zone, 0.0..=3.0).text("dead zone"));
    ui.add(egui::Slider::new(&mut s.chroma_boost, -2..=4).text("chroma boost"));
    ui.horizontal(|ui| {
        for sz in [64u32, 128, 256] {
            if ui.selectable_label(s.size == sz && s.source.name.starts_with("test pattern"), format!("{sz}²")).clicked() {
                s.size = sz; s.source = Source::pattern(sz); s.outcome = None; s.report = None; s.tex = None;
            }
        }
        ui.separator();
        ui.label("image:");
        ui.add(egui::TextEdit::singleline(&mut s.image_path).id_salt("lab-image-path").desired_width(260.0));
        if ui.button("Load").on_hover_text("Load a PNG or JPEG for an encoding test (fao_app's Image button)").clicked() {
            match Source::load(&s.image_path) {
                Ok(src) => { s.source = src; s.load_error = None; s.outcome = None; s.report = None; s.tex = None; }
                Err(e) => s.load_error = Some(e),
            }
        }
    });
    if let Some(e) = &s.load_error { ui.colored_label(ui.visuals().error_fg_color, e); }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("source: {} ({}×{})", s.source.name, s.source.w, s.source.h)).weak().small());
        // EVERY codec is listed; only the ones that write real bytes are
        // selectable, the rest greyed with why. Today that is Keystone alone --
        // see `codec_catalog` for the trace behind each capability.
        crate::codec_catalog::picker(ui, "lab_codec", crate::codec_catalog::Purpose::Encode, &mut s.codec);
        if ui.button("▶ encode").clicked() {
            let (r, o) = run_source(&s.source, s.quality, s.dead_zone, s.chroma_boost);
            s.report = Some(r); s.outcome = Some(o); s.tex = None;
        }
    });
    if let Some(r) = &s.report {
        ui.separator();
        ui.monospace(format!("{} bytes   {:.3} bpp   PSNR {:.2} dB", r.bytes, r.bpp, r.psnr));
    }
    let Some(o) = &s.outcome else { return };
    ui.horizontal(|ui| {
        for (v, label, tip) in [(View::Source, "Orig", "the source frame"), (View::Decoded, "Dec", "the encoded → decoded result"),
                                (View::Error, "Err", "per-pixel |source − decoded|, ×4")] {
            if ui.selectable_label(s.view == v, label).on_hover_text(tip).clicked() { s.view = v; }
        }
    });
    let pixels: &[u8] = match s.view { View::Source => &s.source.rgb, View::Decoded => &o.recon, View::Error => &o.error };
    if s.tex.as_ref().map(|(v, _)| *v != s.view).unwrap_or(true) {
        let img = egui::ColorImage::from_rgb([o.w, o.h], pixels);
        s.tex = Some((s.view, ui.ctx().load_texture("lab-view", img, egui::TextureOptions::NEAREST)));
    }
    if let Some((_, t)) = &s.tex {
        let size = t.size_vec2();
        let avail = ui.available_size();
        let scale = (avail.x / size.x).min(avail.y / size.y).min(4.0).max(0.05);
        ui.image((t.id(), size * scale));
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
        // STRICTLY more: the recon-as-bytes mixup gave w·h·3 for both, and `>=` was satisfied.
        assert!(hi.bytes > lo.bytes, "higher quality must grow the stream (lo={} hi={})", lo.bytes, hi.bytes);
        assert!(hi.bpp < 24.0 && lo.bpp < hi.bpp, "bpp is the stream's, not the reconstruction's (lo={:.3} hi={:.3})", lo.bpp, hi.bpp);
        assert!(hi.psnr > lo.psnr, "higher quality must raise PSNR (lo={:.2} hi={:.2})", lo.psnr, hi.psnr);
        assert!(hi.psnr > 25.0, "q=0.95 on a 64² pattern should exceed 25 dB (got {:.2})", hi.psnr);
    }

    #[test]
    fn the_lab_is_deterministic() {
        assert_eq!(run_lab(64, 0.5, 1.0, 1), run_lab(64, 0.5, 1.0, 1));
        assert_eq!(test_pattern(32), test_pattern(32));
    }

    /// The reported PSNR is computed from the same reconstruction the Decoded view shows.
    #[test]
    fn the_decoded_view_is_the_frame_the_psnr_was_measured_on() {
        let src = Source::pattern(64);
        let (r, o) = run_source(&src, 0.6, 1.0, 0);
        let n = (o.w * o.h * 3) as f64;
        let mse = src.rgb.iter().zip(&o.recon).map(|(a, b)| { let d = *a as f64 - *b as f64; d * d }).sum::<f64>() / n;
        let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
        assert!((psnr - r.psnr).abs() < 1e-6, "{psnr} vs {}", r.psnr);
        assert_eq!(o.error.len(), o.recon.len());
        assert!(o.error.iter().any(|&e| e > 0), "a lossy encode has some error to show");
    }
}
