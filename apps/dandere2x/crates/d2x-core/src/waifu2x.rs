//! waifu2x `upconv_7`: six valid 3×3 convolutions with LeakyReLU(0.1), then
//! a 4×4 stride-2 transposed convolution to RGB. The model files are
//! nagadomi's JSON (`models/upconv_7/{art,photo}/*.json`).
//!
//! Geometry: each conv loses 2 pixels, the deconv maps `n → 2n − 4`, so an
//! input of `n` becomes `2n − 28`. Pre-padding the input by 7 (replicate)
//! yields exactly `2n`. That "offset 14" is recorded in the model's own
//! `model_config`, and this loader checks it.
//!
//! This module is the CPU reference. It is deliberately naive — the GPU
//! implementation in `d2x-wgpu` is checked against it, and an oracle that is
//! obviously correct is worth more than a fast one. It is rayon-parallel
//! over output channels so it is also a tolerable no-GPU fallback for small
//! residual images.

use crate::frame::{Frame, Pad};
use rayon::prelude::*;
use std::path::Path;

pub const LEAKY_SLOPE: f32 = 0.1;

/// A planar `C×H×W` f32 activation.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    pub c: usize,
    pub h: usize,
    pub w: usize,
    pub data: Vec<f32>,
}

impl Tensor {
    pub fn zeros(c: usize, h: usize, w: usize) -> Self {
        Tensor { c, h, w, data: vec![0.0; c * h * w] }
    }
    #[inline]
    pub fn at(&self, c: usize, y: usize, x: usize) -> f32 {
        self.data[(c * self.h + y) * self.w + x]
    }
    pub fn plane(&self, c: usize) -> &[f32] {
        &self.data[c * self.h * self.w..(c + 1) * self.h * self.w]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conv3x3 {
    pub in_ch: usize,
    pub out_ch: usize,
    /// `[out][in][3][3]`
    pub weight: Vec<f32>,
    pub bias: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deconv4x4s2 {
    pub in_ch: usize,
    pub out_ch: usize,
    /// `[in][out][4][4]` (Torch `SpatialFullConvolution` order)
    pub weight: Vec<f32>,
    pub bias: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Upconv7 {
    pub convs: Vec<Conv3x3>,
    pub deconv: Deconv4x4s2,
    pub name: String,
}

/// Input pixels lost per side by the network; the pre-padding that makes
/// the output exactly 2× the input.
pub const PRE_PAD: u32 = 7;

impl Upconv7 {
    pub fn scale(&self) -> u32 {
        2
    }

    /// Output size for an input of `n` (no pre-padding).
    pub fn out_size(n: usize) -> usize {
        2 * (n - 12) - 4
    }

    /// Deterministic random weights of the real shapes, for tests and the
    /// GPU self-test when no model file is at hand. Scaled so activations
    /// stay O(1) through the stack.
    pub fn synthetic(seed: u64) -> Self {
        let shapes = [(3, 16), (16, 32), (32, 64), (64, 128), (128, 128), (128, 256)];
        let mut s = seed ^ 0xD1B5_4A32_D192_ED03;
        let mut next = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / (1u64 << 53) as f64) as f32 * 2.0 - 1.0
        };
        let convs = shapes
            .iter()
            .map(|&(i, o)| {
                let fan_in = (i * 9) as f32;
                let scale = (2.0 / fan_in).sqrt();
                Conv3x3 {
                    in_ch: i,
                    out_ch: o,
                    weight: (0..o * i * 9).map(|_| next() * scale).collect(),
                    bias: (0..o).map(|_| next() * 0.05).collect(),
                }
            })
            .collect();
        let deconv = Deconv4x4s2 {
            in_ch: 256,
            out_ch: 3,
            weight: (0..256 * 3 * 16).map(|_| next() * (2.0f32 / (256.0 * 4.0)).sqrt()).collect(),
            bias: (0..3).map(|_| 0.5 + next() * 0.05).collect(),
        };
        Upconv7 { convs, deconv, name: format!("synthetic(seed {seed})") }
    }

    pub fn load_json(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Self::from_json(&v, name)
    }

    pub fn from_json(v: &serde_json::Value, name: String) -> anyhow::Result<Self> {
        let layers = v.as_array().ok_or_else(|| anyhow::anyhow!("model json: expected a top-level array of layers"))?;
        anyhow::ensure!(layers.len() == 7, "upconv_7 has 7 layers, file has {}", layers.len());

        if let Some(cfg) = layers[0].get("model_config") {
            let arch = cfg.get("arch_name").and_then(|a| a.as_str()).unwrap_or("?");
            anyhow::ensure!(arch == "upconv_7", "model arch is {arch}, only upconv_7 is implemented");
            let offset = cfg.get("offset").and_then(|o| o.as_u64()).unwrap_or(0);
            anyhow::ensure!(offset == 14, "model offset {offset} != 14 (pre-pad {PRE_PAD} assumes 14)");
            let ch = cfg.get("channels").and_then(|c| c.as_u64()).unwrap_or(3);
            anyhow::ensure!(ch == 3, "model channels {ch}, only RGB models supported");
        }

        fn flatten(v: &serde_json::Value, out: &mut Vec<f32>) -> anyhow::Result<()> {
            match v {
                serde_json::Value::Array(a) => {
                    for x in a {
                        flatten(x, out)?;
                    }
                    Ok(())
                }
                serde_json::Value::Number(n) => {
                    out.push(n.as_f64().ok_or_else(|| anyhow::anyhow!("non-float weight"))? as f32);
                    Ok(())
                }
                _ => anyhow::bail!("unexpected value in weight tensor"),
            }
        }
        fn field(l: &serde_json::Value, k: &str) -> anyhow::Result<usize> {
            l.get(k).and_then(|x| x.as_u64()).map(|x| x as usize).ok_or_else(|| anyhow::anyhow!("layer missing {k}"))
        }

        let mut convs = Vec::with_capacity(6);
        for (i, l) in layers.iter().take(6).enumerate() {
            let class = l.get("class_name").and_then(|c| c.as_str()).unwrap_or("");
            anyhow::ensure!(class == "nn.SpatialConvolutionMM", "layer {i}: {class}, expected SpatialConvolutionMM");
            let (in_ch, out_ch) = (field(l, "nInputPlane")?, field(l, "nOutputPlane")?);
            anyhow::ensure!(field(l, "kW")? == 3 && field(l, "kH")? == 3, "layer {i}: kernel is not 3x3");
            let mut weight = Vec::with_capacity(out_ch * in_ch * 9);
            flatten(l.get("weight").ok_or_else(|| anyhow::anyhow!("layer {i}: no weight"))?, &mut weight)?;
            anyhow::ensure!(weight.len() == out_ch * in_ch * 9, "layer {i}: weight has {} values, expected {}", weight.len(), out_ch * in_ch * 9);
            let mut bias = Vec::with_capacity(out_ch);
            flatten(l.get("bias").ok_or_else(|| anyhow::anyhow!("layer {i}: no bias"))?, &mut bias)?;
            anyhow::ensure!(bias.len() == out_ch, "layer {i}: bias length");
            if let Some(prev) = convs.last() {
                let p: &Conv3x3 = prev;
                anyhow::ensure!(p.out_ch == in_ch, "layer {i}: in_ch {in_ch} != previous out_ch {}", p.out_ch);
            } else {
                anyhow::ensure!(in_ch == 3, "layer 0: in_ch {in_ch} != 3");
            }
            convs.push(Conv3x3 { in_ch, out_ch, weight, bias });
        }

        let l = &layers[6];
        let class = l.get("class_name").and_then(|c| c.as_str()).unwrap_or("");
        anyhow::ensure!(class == "nn.SpatialFullConvolution", "layer 6: {class}, expected SpatialFullConvolution");
        let (in_ch, out_ch) = (field(l, "nInputPlane")?, field(l, "nOutputPlane")?);
        anyhow::ensure!(
            field(l, "kW")? == 4 && field(l, "kH")? == 4 && field(l, "dW")? == 2 && field(l, "dH")? == 2 && field(l, "padW")? == 3 && field(l, "padH")? == 3,
            "layer 6: expected 4x4 stride 2 pad 3"
        );
        anyhow::ensure!(in_ch == convs[5].out_ch && out_ch == 3, "layer 6: shape {in_ch}->{out_ch}");
        let mut weight = Vec::with_capacity(in_ch * out_ch * 16);
        flatten(l.get("weight").ok_or_else(|| anyhow::anyhow!("layer 6: no weight"))?, &mut weight)?;
        anyhow::ensure!(weight.len() == in_ch * out_ch * 16, "layer 6: weight length {}", weight.len());
        let mut bias = Vec::with_capacity(3);
        flatten(l.get("bias").ok_or_else(|| anyhow::anyhow!("layer 6: no bias"))?, &mut bias)?;
        anyhow::ensure!(bias.len() == 3, "layer 6: bias length");

        Ok(Upconv7 { convs, deconv: Deconv4x4s2 { in_ch, out_ch, weight, bias }, name })
    }

    /// Forward pass on a pre-padded tile. Input `3×h×w` in `[0, 1]`,
    /// output `3×(2h−28)×(2w−28)` clamped to `[0, 1]`.
    pub fn forward(&self, input: &Tensor) -> Tensor {
        assert_eq!(input.c, 3);
        assert!(input.h > 14 && input.w > 14, "tile too small for upconv_7: {}x{}", input.w, input.h);
        let mut act = input.clone();
        for conv in &self.convs {
            act = conv3x3_valid_leaky(conv, &act);
        }
        deconv4x4s2(&self.deconv, &act)
    }
}

pub fn conv3x3_valid_leaky(l: &Conv3x3, x: &Tensor) -> Tensor {
    assert_eq!(x.c, l.in_ch);
    let (oh, ow) = (x.h - 2, x.w - 2);
    let mut out = Tensor::zeros(l.out_ch, oh, ow);
    out.data.par_chunks_mut(oh * ow).enumerate().for_each(|(oc, plane)| {
        for v in plane.iter_mut() {
            *v = l.bias[oc];
        }
        for ic in 0..l.in_ch {
            let xp = x.plane(ic);
            for ky in 0..3 {
                for kx in 0..3 {
                    let wv = l.weight[((oc * l.in_ch + ic) * 3 + ky) * 3 + kx];
                    for oy in 0..oh {
                        let src = &xp[(oy + ky) * x.w + kx..(oy + ky) * x.w + kx + ow];
                        let dst = &mut plane[oy * ow..(oy + 1) * ow];
                        for (d, &s) in dst.iter_mut().zip(src) {
                            *d += wv * s;
                        }
                    }
                }
            }
        }
        for v in plane.iter_mut() {
            if *v < 0.0 {
                *v *= LEAKY_SLOPE;
            }
        }
    });
    out
}

/// Transposed 4×4 stride-2 pad-3 convolution in scatter form, then clamp.
pub fn deconv4x4s2(l: &Deconv4x4s2, x: &Tensor) -> Tensor {
    assert_eq!(x.c, l.in_ch);
    let (oh, ow) = (2 * x.h - 4, 2 * x.w - 4);
    let mut out = Tensor::zeros(l.out_ch, oh, ow);
    out.data.par_chunks_mut(oh * ow).enumerate().for_each(|(oc, plane)| {
        for v in plane.iter_mut() {
            *v = l.bias[oc];
        }
        for ic in 0..l.in_ch {
            let xp = x.plane(ic);
            for iy in 0..x.h {
                for ix in 0..x.w {
                    let s = xp[iy * x.w + ix];
                    for ky in 0..4 {
                        let oy = 2 * iy as i64 - 3 + ky as i64;
                        if oy < 0 || oy >= oh as i64 {
                            continue;
                        }
                        for kx in 0..4 {
                            let ox = 2 * ix as i64 - 3 + kx as i64;
                            if ox < 0 || ox >= ow as i64 {
                                continue;
                            }
                            let wv = l.weight[((ic * l.out_ch + oc) * 4 + ky) * 4 + kx];
                            plane[oy as usize * ow + ox as usize] += wv * s;
                        }
                    }
                }
            }
        }
        for v in plane.iter_mut() {
            *v = v.clamp(0.0, 1.0);
        }
    });
    out
}

/// Round a `[0,1]` float to a byte the same way WGSL's `pack4x8unorm` does
/// (`floor(0.5 + 255·clamp(v))`), so CPU and GPU bytes can be compared exactly.
#[inline]
pub fn unorm8(v: f32) -> u8 {
    (0.5 + 255.0 * v.clamp(0.0, 1.0)).floor() as u8
}

// ---------------------------------------------------------------------------
// Tiling shared by every backend.

/// One tile of the *unpadded* input image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Split a `width×height` image into tiles of at most `tile` on a side.
pub fn tiles(width: u32, height: u32, tile: u32) -> Vec<TileRect> {
    assert!(tile > 0);
    let mut v = Vec::new();
    let mut y = 0;
    while y < height {
        let h = tile.min(height - y);
        let mut x = 0;
        while x < width {
            let w = tile.min(width - x);
            v.push(TileRect { x, y, w, h });
            x += w;
        }
        y += h;
    }
    v
}

/// Extract the pre-padded input for `rect` from a frame that has already
/// been replicate-padded by [`PRE_PAD`], as a `3×(h+14)×(w+14)` tensor in
/// `[0, 1]`.
pub fn extract_tile(padded: &Frame, rect: TileRect) -> Tensor {
    let (tw, th) = ((rect.w + 2 * PRE_PAD) as usize, (rect.h + 2 * PRE_PAD) as usize);
    let mut t = Tensor::zeros(3, th, tw);
    for y in 0..th {
        let row = padded.row(rect.y + y as u32, rect.x, rect.x + tw as u32);
        for x in 0..tw {
            for c in 0..3 {
                t.data[(c * th + y) * tw + x] = row[x * 3 + c] as f32 / 255.0;
            }
        }
    }
    t
}

/// Write a `3×(2h)×(2w)` output tensor for `rect` into the 2× output frame.
pub fn write_tile(out: &mut Frame, rect: TileRect, t: &Tensor) {
    assert_eq!((t.c, t.h as u32, t.w as u32), (3, rect.h * 2, rect.w * 2));
    for y in 0..t.h {
        let oy = rect.y * 2 + y as u32;
        let base = out.idx(rect.x * 2, oy);
        for x in 0..t.w {
            for c in 0..3 {
                out.data[base + x * 3 + c] = unorm8(t.at(c, y, x));
            }
        }
    }
}

/// Write a tile given as packed RGBA8 (what the GPU deconv emits).
pub fn write_tile_rgba8(out: &mut Frame, rect: TileRect, rgba: &[u8]) {
    let (w, h) = ((rect.w * 2) as usize, (rect.h * 2) as usize);
    assert_eq!(rgba.len(), w * h * 4);
    for y in 0..h {
        let base = out.idx(rect.x * 2, rect.y * 2 + y as u32);
        for x in 0..w {
            let s = (y * w + x) * 4;
            out.data[base + x * 3..base + x * 3 + 3].copy_from_slice(&rgba[s..s + 3]);
        }
    }
}

/// Whole-frame upscale on the CPU, tiled. Slow; the fallback and the oracle.
pub fn upscale_cpu(model: &Upconv7, frame: &Frame, tile: u32) -> Frame {
    let padded = frame.padded(PRE_PAD, Pad::Replicate);
    let mut out = Frame::new(frame.width * 2, frame.height * 2);
    for rect in tiles(frame.width, frame.height, tile) {
        let input = extract_tile(&padded, rect);
        let y = model.forward(&input);
        write_tile(&mut out, rect, &y);
    }
    out
}

/// CPU fallback [`crate::upscale::Upscaler`].
pub struct CpuWaifu2x {
    pub model: Upconv7,
    pub tile: u32,
}

impl crate::upscale::Upscaler for CpuWaifu2x {
    fn scale(&self) -> u32 {
        2
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        Ok(upscale_cpu(&self.model, frame, self.tile))
    }
    fn name(&self) -> String {
        format!("waifu2x-cpu [{}] tile {}", self.model.name, self.tile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_out_size() {
        assert_eq!(Upconv7::out_size(24), 20);
        assert_eq!(Upconv7::out_size(14 + 10), 20);
    }

    #[test]
    fn forward_shapes_and_range() {
        let m = Upconv7::synthetic(1);
        let mut x = Tensor::zeros(3, 24, 30);
        for (i, v) in x.data.iter_mut().enumerate() {
            *v = ((i * 7919) % 1000) as f32 / 1000.0;
        }
        let y = m.forward(&x);
        assert_eq!((y.c, y.h, y.w), (3, 20, 32));
        assert!(y.data.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(y.data.iter().any(|&v| v > 0.0 && v < 1.0), "output should not be all-saturated");
    }

    #[test]
    fn tiles_cover_exactly() {
        let ts = tiles(100, 45, 32);
        let area: u32 = ts.iter().map(|t| t.w * t.h).sum();
        assert_eq!(area, 100 * 45);
        assert_eq!(ts.len(), 4 * 2);
        assert_eq!(ts.last().unwrap(), &TileRect { x: 96, y: 32, w: 4, h: 13 });
    }

    /// Tiling must not change the result: the network is translation
    /// equivariant and tiles are cut from the same padded image.
    #[test]
    fn tiled_equals_untiled() {
        let m = Upconv7::synthetic(2);
        let mut f = Frame::new(20, 18);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = ((i * 37) % 256) as u8;
        }
        let a = upscale_cpu(&m, &f, 64);
        let b = upscale_cpu(&m, &f, 7);
        assert_eq!(a, b);
        assert_eq!((a.width, a.height), (40, 36));
    }

    #[test]
    fn unorm8_rounds_like_pack4x8unorm() {
        assert_eq!(unorm8(0.0), 0);
        assert_eq!(unorm8(1.0), 255);
        assert_eq!(unorm8(0.5), 128);
        assert_eq!(unorm8(-3.0), 0);
        assert_eq!(unorm8(1.0 / 255.0), 1);
    }

    #[test]
    fn loads_real_model_if_present() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models/upconv_7_art_scale2.0x_model.json");
        if !p.exists() {
            eprintln!("skipping: {} not present", p.display());
            return;
        }
        let m = Upconv7::load_json(&p).unwrap();
        assert_eq!(m.convs.len(), 6);
        assert_eq!(m.convs[0].in_ch, 3);
        assert_eq!(m.convs[5].out_ch, 256);
        assert_eq!(m.deconv.weight.len(), 256 * 3 * 16);
        // A flat mid-grey tile must come out near mid-grey: a loader that
        // scrambled the weight order would not.
        let mut x = Tensor::zeros(3, 30, 30);
        x.data.iter_mut().for_each(|v| *v = 0.5);
        let y = m.forward(&x);
        let mean = y.data.iter().sum::<f32>() / y.data.len() as f32;
        assert!((mean - 0.5).abs() < 0.05, "mean {mean}");
    }
}
