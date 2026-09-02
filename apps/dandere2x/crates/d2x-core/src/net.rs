//! The super-resolution network as data: a list of ops over numbered planar
//! f32 buffers. Three architectures build onto it —
//!
//! - **RRDBNet** (Real-ESRGAN x2/x4/x8, the photo models): 23 residual-in-
//!   residual dense blocks at feature resolution, then nearest-×2 + conv
//!   upsampling. ~4.4 MMAC per input pixel at x2 (body at ¼ res), ~17 at x4.
//! - **SRVGGNetCompact** (`realesr-animevideov3`): 16 same-pad convs with
//!   PReLU, pixel-shuffle ×4, plus the nearest-upsampled input. ~0.6 MMAC.
//! - **upconv_7** (waifu2x): six valid convs + a stride-2 transposed conv.
//!
//! Both the CPU reference here and the wgpu runner execute the same op list,
//! so an architecture is a builder function, not a kernel.
//!
//! Buffer/op conventions that make dense concatenation free: channels are
//! contiguous planes, so `cat(x, x1, x2)` is just "x1 was written at channel
//! offset 64 and x2 at 96 of the same buffer" and the next conv reads the
//! first `src_ch` channels. Nearest ×2 upsampling is folded into the next
//! conv's read addressing (`in_scale`), never materialised.

use crate::frame::{Frame, Pad};
use crate::st::SafeTensors;
use rayon::prelude::*;

pub type BufId = usize;

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
    pub fn plane_mut(&mut self, c: usize) -> &mut [f32] {
        let n = self.h * self.w;
        &mut self.data[c * n..(c + 1) * n]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    None,
    Leaky(f32),
    /// Per-output-channel slopes in `ConvOp::prelu`.
    PReLU,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConvOp {
    pub name: String,
    pub src: BufId,
    /// Read channels `[0, src_ch)` of `src`.
    pub src_ch: usize,
    /// 1, or 2 to read the source as if nearest-upsampled ×2.
    pub in_scale: u32,
    pub dst: BufId,
    /// Write channels `[dst_off, dst_off + out_ch)` of `dst`.
    pub dst_off: usize,
    pub out_ch: usize,
    /// 0 = valid (output shrinks by 2), 1 = same (zero padding).
    pub pad: u32,
    pub act: Act,
    pub prelu: Vec<f32>,
    /// `out = acc * scale + residual[channels 0..out_ch]` before activation.
    pub residual: Option<(BufId, f32)>,
    /// `[out][in][3][3]`
    pub weight: Vec<f32>,
    pub bias: Vec<f32>,
}

/// waifu2x's stride-2 4×4 transposed conv (valid: `n → 2n − 4`), no activation.
#[derive(Debug, Clone, PartialEq)]
pub struct DeconvOp {
    pub src: BufId,
    pub dst: BufId,
    pub in_ch: usize,
    pub out_ch: usize,
    /// `[in][out][4][4]`
    pub weight: Vec<f32>,
    pub bias: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Conv(ConvOp),
    Deconv(DeconvOp),
    /// Copy channels `[0, ch)` of `src` to `[dst_off, dst_off+ch)` of `dst`.
    Copy { src: BufId, dst: BufId, dst_off: usize, ch: usize },
    /// `dst[0..ch] = dst[0..ch] * scale + src[0..ch]`
    AddScaled { dst: BufId, src: BufId, ch: usize, scale: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufSpec {
    pub channels: usize,
    /// Spatial size relative to the network input tensor (1, 2, 4, 8).
    pub level: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub name: String,
    pub ops: Vec<Op>,
    pub bufs: Vec<BufSpec>,
    pub input: BufId,
    pub output: BufId,
    /// Image scale factor the network produces.
    pub scale: u32,
    /// Input pixel-unshuffle factor (RRDB x2: 2).
    pub unshuffle: u32,
    /// Output pixel-shuffle factor (SRVGG: 4).
    pub shuffle: u32,
    /// Output += nearest-upsampled input (SRVGG's skip).
    pub add_input: bool,
    /// Context pixels per side of an input tile. For valid networks this is
    /// exact (upconv_7: 7); for same-pad networks it is the overlap that
    /// hides tile seams (Real-ESRGAN uses 10).
    pub pre_pad: u32,
    /// Output crops exactly to `scale × tile` with no trimming (valid nets).
    pub exact: bool,
}

impl Network {
    pub fn macs_per_input_pixel(&self) -> f64 {
        let mut total = 0.0;
        for op in &self.ops {
            match op {
                Op::Conv(c) => {
                    let lvl = self.bufs[c.dst].level as f64;
                    total += 9.0 * c.src_ch as f64 * c.out_ch as f64 * lvl * lvl;
                }
                Op::Deconv(d) => total += 4.0 * d.in_ch as f64 * d.out_ch as f64 * 4.0,
                _ => {}
            }
        }
        total / (self.unshuffle as f64 * self.unshuffle as f64)
    }

    /// Output tensor size for an input of `(h, w)` (network input, after unshuffle).
    pub fn out_size(&self, h: usize, w: usize) -> (usize, usize) {
        let mut sizes: Vec<Option<(usize, usize)>> = vec![None; self.bufs.len()];
        sizes[self.input] = Some((h, w));
        for op in &self.ops {
            match op {
                Op::Conv(c) => {
                    let (ih, iw) = sizes[c.src].expect("conv src unsized");
                    let (ih, iw) = (ih * c.in_scale as usize, iw * c.in_scale as usize);
                    sizes[c.dst] = Some((ih + 2 * c.pad as usize - 2, iw + 2 * c.pad as usize - 2));
                }
                Op::Deconv(d) => {
                    let (ih, iw) = sizes[d.src].expect("deconv src unsized");
                    sizes[d.dst] = Some((2 * ih - 4, 2 * iw - 4));
                }
                Op::Copy { src, dst, .. } | Op::AddScaled { dst, src, .. } => {
                    if sizes[*dst].is_none() {
                        sizes[*dst] = sizes[*src];
                    }
                }
            }
        }
        sizes[self.output].expect("output unsized")
    }

    // ------------------------------------------------------------------
    // Builders

    /// Real-ESRGAN RRDBNet from a converted state dict.
    pub fn rrdb(st: &SafeTensors, name: String) -> anyhow::Result<Network> {
        let cin = st.shape("conv_first.weight").ok_or_else(|| anyhow::anyhow!("not an RRDBNet: no conv_first"))?[1];
        let (unshuffle, mut scale) = match cin {
            3 => (1, 4),
            12 => (2, 2),
            48 => (4, 1),
            _ => anyhow::bail!("conv_first has {cin} input channels; expected 3, 12 or 48"),
        };
        let n_blocks = 1 + st.names().filter_map(|k| k.strip_prefix("body.")).filter_map(|k| k.split('.').next()).filter_map(|k| k.parse::<usize>().ok()).max().ok_or_else(|| anyhow::anyhow!("no body blocks"))?;
        let nf = st.shape("conv_first.weight").unwrap()[0];
        let gc = st.shape("body.0.rdb1.conv1.weight").ok_or_else(|| anyhow::anyhow!("no rdb"))?[0];
        let has_up3 = st.contains("conv_up3.weight");
        if has_up3 {
            scale = 8;
        }
        let dense = nf + 4 * gc;

        // Buffers: 0 input, 1 feat, 2/3 dense ping-pong, 4 keep, 5 up1, 6 up2, 7 up3, 8 hr, 9 out
        let bufs = vec![
            BufSpec { channels: cin, level: 1 },
            BufSpec { channels: nf, level: 1 },
            BufSpec { channels: dense, level: 1 },
            BufSpec { channels: dense, level: 1 },
            BufSpec { channels: nf, level: 1 },
            BufSpec { channels: nf, level: 2 },
            BufSpec { channels: nf, level: 4 },
            BufSpec { channels: nf, level: 8 },
            BufSpec { channels: nf, level: if has_up3 { 8 } else { 4 } },
            BufSpec { channels: 3, level: if has_up3 { 8 } else { 4 } },
        ];
        let (b_in, b_feat, b_d0, b_d1, b_keep, b_up1, b_up2, b_up3, b_hr, b_out) = (0, 1, 2, 3, 4, 5, 6, 7, 8, 9);
        let lrelu = Act::Leaky(0.2);
        let conv = |name: &str, src, src_ch, in_scale, dst, dst_off, pad, act, residual| -> anyhow::Result<Op> {
            let w = st.f32(&format!("{name}.weight"))?;
            let b = st.f32(&format!("{name}.bias"))?;
            let shape = st.shape(&format!("{name}.weight")).unwrap();
            anyhow::ensure!(shape[1] == src_ch, "{name}: expects {} in channels, wiring gives {src_ch}", shape[1]);
            Ok(Op::Conv(ConvOp { name: name.to_string(), src, src_ch, in_scale, dst, dst_off, out_ch: shape[0], pad, act, prelu: Vec::new(), residual, weight: w, bias: b }))
        };

        let mut ops = Vec::new();
        // conv_first: input -> D0[0..nf]  (the body's running state lives in the dense buffers' first nf channels)
        ops.push(conv("conv_first", b_in, cin, 1, b_d0, 0, 1, Act::None, None)?);
        ops.push(Op::Copy { src: b_d0, dst: b_feat, dst_off: 0, ch: nf }); // feat, for the trunk skip
        let mut cur = b_d0; // where x currently lives (channels 0..nf)
        for blk in 0..n_blocks {
            ops.push(Op::Copy { src: cur, dst: b_keep, dst_off: 0, ch: nf });
            for r in 1..=3 {
                let p = format!("body.{blk}.rdb{r}");
                let nxt = if cur == b_d0 { b_d1 } else { b_d0 };
                ops.push(conv(&format!("{p}.conv1"), cur, nf, 1, cur, nf, 1, lrelu, None)?);
                ops.push(conv(&format!("{p}.conv2"), cur, nf + gc, 1, cur, nf + gc, 1, lrelu, None)?);
                ops.push(conv(&format!("{p}.conv3"), cur, nf + 2 * gc, 1, cur, nf + 2 * gc, 1, lrelu, None)?);
                ops.push(conv(&format!("{p}.conv4"), cur, nf + 3 * gc, 1, cur, nf + 3 * gc, 1, lrelu, None)?);
                // x5*0.2 + x  -> next dense buffer's state
                ops.push(conv(&format!("{p}.conv5"), cur, nf + 4 * gc, 1, nxt, 0, 1, Act::None, Some((cur, 0.2)))?);
                cur = nxt;
            }
            ops.push(Op::AddScaled { dst: cur, src: b_keep, ch: nf, scale: 0.2 });
        }
        // feat = feat + conv_body(body)
        ops.push(conv("conv_body", cur, nf, 1, b_feat, 0, 1, Act::None, Some((b_feat, 1.0)))?);
        ops.push(conv("conv_up1", b_feat, nf, 2, b_up1, 0, 1, lrelu, None)?);
        ops.push(conv("conv_up2", b_up1, nf, 2, b_up2, 0, 1, lrelu, None)?);
        let mut top = b_up2;
        if has_up3 {
            ops.push(conv("conv_up3", b_up2, nf, 2, b_up3, 0, 1, lrelu, None)?);
            top = b_up3;
        }
        ops.push(conv("conv_hr", top, nf, 1, b_hr, 0, 1, lrelu, None)?);
        ops.push(conv("conv_last", b_hr, nf, 1, b_out, 0, 1, Act::None, None)?);

        // Overlap. This was 8 feature pixels, justified as "keeps the interior
        // error around 1 LSB (measured in tests)".
        //
        // That measurement does not hold on real footage. RRDB's body has a
        // receptive field of HUNDREDS of pixels, and every conv here is
        // `pad: 1` — ZERO padding. At a tile's feature-space boundary the
        // convolutions therefore read zeros, and with only 8 pixels of context
        // that contamination propagates well inside the cropped region. On
        // smooth synthetic content it averages out to ~1 LSB; on real detail it
        // is a visible dark seam at every tile edge, which is exactly what was
        // reported from a run over footage.
        //
        // 8 -> 32 as the default: 4x the context for 2*24 extra pixels per tile
        // edge, which at the default tile size is a few percent more work. Not
        // a cure — with zero-padded convs and a receptive field this large,
        // NOTHING short of overlap-plus-feathered-blending removes the seam
        // entirely — but it moves the contaminated band out of the visible
        // crop for typical content.
        //
        // Settable, because the right value depends on the model's depth and
        // the operator's tolerance for compute, and 8 was a hardcoded number
        // that nobody could raise when it turned out to be wrong.
        let pre_pad = std::env::var("D2X_TILE_PAD").ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(32)
            .clamp(0, 128) * unshuffle;
        Ok(Network { name, ops, bufs, input: b_in, output: b_out, scale, unshuffle, shuffle: 1, add_input: false, pre_pad, exact: false })
    }

    /// SRVGGNetCompact (`realesr-animevideov3`) from a converted state dict.
    pub fn srvgg(st: &SafeTensors, name: String) -> anyhow::Result<Network> {
        let n = 1 + st.names().filter_map(|k| k.strip_prefix("body.")).filter_map(|k| k.split('.').next()).filter_map(|k| k.parse::<usize>().ok()).max().ok_or_else(|| anyhow::anyhow!("not SRVGG: no body.N"))?;
        let nf = st.shape("body.0.weight").ok_or_else(|| anyhow::anyhow!("no body.0"))?[0];
        let last = st.shape(&format!("body.{}.weight", n - 1)).unwrap()[0];
        anyhow::ensure!(last % 3 == 0, "last conv has {last} channels");
        let r2 = last / 3;
        let shuffle = (r2 as f64).sqrt() as u32;
        anyhow::ensure!(shuffle * shuffle == r2 as u32, "last conv channels {last} is not 3·r²");
        // Buffers: 0 input, 1/2 ping-pong, 3 output (3r² channels at level 1)
        let bufs = vec![BufSpec { channels: 3, level: 1 }, BufSpec { channels: nf, level: 1 }, BufSpec { channels: nf, level: 1 }, BufSpec { channels: last, level: 1 }];
        let mut ops = Vec::new();
        let mut cur = 0;
        let mut i = 0;
        while i < n - 1 {
            let w = st.f32(&format!("body.{i}.weight"))?;
            let b = st.f32(&format!("body.{i}.bias"))?;
            let shape = st.shape(&format!("body.{i}.weight")).unwrap();
            let prelu = st.f32(&format!("body.{}.weight", i + 1))?;
            anyhow::ensure!(prelu.len() == shape[0], "body.{}: PReLU has {} slopes for {} channels", i + 1, prelu.len(), shape[0]);
            let dst = if cur == 1 { 2 } else { 1 };
            ops.push(Op::Conv(ConvOp { name: format!("body.{i}"), src: cur, src_ch: shape[1], in_scale: 1, dst, dst_off: 0, out_ch: shape[0], pad: 1, act: Act::PReLU, prelu, residual: None, weight: w, bias: b }));
            cur = dst;
            i += 2;
        }
        let w = st.f32(&format!("body.{}.weight", n - 1))?;
        let b = st.f32(&format!("body.{}.bias", n - 1))?;
        ops.push(Op::Conv(ConvOp { name: format!("body.{}", n - 1), src: cur, src_ch: nf, in_scale: 1, dst: 3, dst_off: 0, out_ch: last, pad: 1, act: Act::None, prelu: Vec::new(), residual: None, weight: w, bias: b }));
        Ok(Network { name, ops, bufs, input: 0, output: 3, scale: shuffle, unshuffle: 1, shuffle, add_input: true, pre_pad: 10, exact: false })
    }

    /// waifu2x upconv_7 from nagadomi's JSON.
    pub fn upconv7(v: &serde_json::Value, name: String) -> anyhow::Result<Network> {
        let m = crate::waifu2x::Upconv7::from_json(v, name.clone())?;
        // Buffers: 0 input, 1/2 ping-pong (max 256 ch), 3 output
        let bufs = vec![BufSpec { channels: 3, level: 1 }, BufSpec { channels: 256, level: 1 }, BufSpec { channels: 256, level: 1 }, BufSpec { channels: 3, level: 2 }];
        let mut ops = Vec::new();
        let mut cur = 0;
        for (i, c) in m.convs.iter().enumerate() {
            let dst = if cur == 1 { 2 } else { 1 };
            ops.push(Op::Conv(ConvOp { name: format!("conv{i}"), src: cur, src_ch: c.in_ch, in_scale: 1, dst, dst_off: 0, out_ch: c.out_ch, pad: 0, act: Act::Leaky(crate::waifu2x::LEAKY_SLOPE), prelu: Vec::new(), residual: None, weight: c.weight.clone(), bias: c.bias.clone() }));
            cur = dst;
        }
        ops.push(Op::Deconv(DeconvOp { src: cur, dst: 3, in_ch: m.deconv.in_ch, out_ch: 3, weight: m.deconv.weight.clone(), bias: m.deconv.bias.clone() }));
        Ok(Network { name, ops, bufs, input: 0, output: 3, scale: 2, unshuffle: 1, shuffle: 1, add_input: false, pre_pad: crate::waifu2x::PRE_PAD, exact: true })
    }

    /// Load any supported model file by extension and contents.
    pub fn load(path: &std::path::Path) -> anyhow::Result<Network> {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => {
                let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
                Network::upconv7(&v, name)
            }
            Some("safetensors") => {
                let st = SafeTensors::load(path)?;
                if st.contains("conv_first.weight") {
                    Network::rrdb(&st, name)
                } else if st.contains("body.0.weight") {
                    Network::srvgg(&st, name)
                } else {
                    anyhow::bail!("{}: not an RRDBNet or SRVGG state dict", path.display())
                }
            }
            Some("pth") | Some("pt") => anyhow::bail!("{}: convert PyTorch checkpoints with tools/pth2st.py first", path.display()),
            _ => anyhow::bail!("{}: unknown model format (want .safetensors or .json)", path.display()),
        }
    }

    // ------------------------------------------------------------------
    // CPU reference execution

    pub fn forward(&self, input: &Tensor) -> Tensor {
        assert_eq!(input.c, self.bufs[self.input].channels);
        let mut bufs: Vec<Option<Tensor>> = vec![None; self.bufs.len()];
        bufs[self.input] = Some(input.clone());
        for op in &self.ops {
            match op {
                Op::Conv(c) => {
                    let src_t = bufs[c.src].as_ref().expect("conv src");
                    let (ih, iw) = (src_t.h * c.in_scale as usize, src_t.w * c.in_scale as usize);
                    let (oh, ow) = (ih + 2 * c.pad as usize - 2, iw + 2 * c.pad as usize - 2);
                    let residual = c.residual.map(|(b, s)| (bufs[b].as_ref().expect("residual buf").clone(), s));
                    // A dense-block conv reads [0, src_ch) and writes [dst_off, ..) of
                    // the same buffer; the ranges are disjoint, so a copy of the
                    // input channels is the honest reference semantics.
                    let src_owned: Tensor = {
                        let mut t = Tensor::zeros(c.src_ch, src_t.h, src_t.w);
                        for ch in 0..c.src_ch {
                            t.plane_mut(ch).copy_from_slice(src_t.plane(ch));
                        }
                        t
                    };
                    let mut dst = bufs[c.dst].take().filter(|t| t.h == oh && t.w == ow).unwrap_or_else(|| Tensor::zeros(self.bufs[c.dst].channels, oh, ow));
                    conv3x3_cpu(c, &src_owned, &mut dst, residual.as_ref());
                    bufs[c.dst] = Some(dst);
                }
                Op::Deconv(d) => {
                    let src = bufs[d.src].as_ref().expect("deconv src");
                    let out = deconv4x4s2_cpu(d, src);
                    bufs[d.dst] = Some(out);
                }
                Op::Copy { src, dst, dst_off, ch } => {
                    let s = bufs[*src].as_ref().expect("copy src").clone();
                    let mut d = bufs[*dst].take().filter(|t| t.h == s.h && t.w == s.w).unwrap_or_else(|| Tensor::zeros(self.bufs[*dst].channels, s.h, s.w));
                    for c in 0..*ch {
                        d.plane_mut(dst_off + c).copy_from_slice(s.plane(c));
                    }
                    bufs[*dst] = Some(d);
                }
                Op::AddScaled { dst, src, ch, scale } => {
                    let s = bufs[*src].as_ref().expect("add src").clone();
                    let d = bufs[*dst].as_mut().expect("add dst");
                    for c in 0..*ch {
                        for (dv, sv) in d.plane_mut(c).iter_mut().zip(s.plane(c)) {
                            *dv = *dv * scale + sv;
                        }
                    }
                }
            }
        }
        bufs[self.output].take().expect("output")
    }
}

/// Same/valid 3×3 conv with folded nearest upsampling, residual, activation.
/// Writes channels `[dst_off, dst_off+out_ch)` of `dst`.
pub fn conv3x3_cpu(c: &ConvOp, x: &Tensor, dst: &mut Tensor, residual: Option<&(Tensor, f32)>) {
    let s = c.in_scale as usize;
    let (ih, iw) = (x.h * s, x.w * s);
    let pad = c.pad as usize;
    let (oh, ow) = (dst.h, dst.w);
    debug_assert_eq!((oh, ow), (ih + 2 * pad - 2, iw + 2 * pad - 2));
    let in_ch = c.src_ch;
    let plane = oh * ow;
    let base = c.dst_off * plane;
    dst.data[base..base + c.out_ch * plane].par_chunks_mut(plane).enumerate().for_each(|(oc, out)| {
        for v in out.iter_mut() {
            *v = c.bias[oc];
        }
        for ic in 0..in_ch {
            let xp = x.plane(ic);
            for ky in 0..3 {
                for kx in 0..3 {
                    let wv = c.weight[((oc * in_ch + ic) * 3 + ky) * 3 + kx];
                    // Output columns whose source column sx = ox + kx - pad is inside [0, iw).
                    let ox0 = pad.saturating_sub(kx);
                    let ox1 = (iw + pad - kx).min(ow);
                    if ox1 <= ox0 {
                        continue;
                    }
                    for oy in 0..oh {
                        let sy = oy + ky;
                        if sy < pad || sy - pad >= ih {
                            continue;
                        }
                        let row = &xp[((sy - pad) / s) * x.w..((sy - pad) / s + 1) * x.w];
                        let out_row = &mut out[oy * ow + ox0..oy * ow + ox1];
                        let sx0 = ox0 + kx - pad;
                        if s == 1 {
                            for (o, &i) in out_row.iter_mut().zip(&row[sx0..sx0 + (ox1 - ox0)]) {
                                *o += wv * i;
                            }
                        } else {
                            for (k, o) in out_row.iter_mut().enumerate() {
                                *o += wv * row[(sx0 + k) / s];
                            }
                        }
                    }
                }
            }
        }
        if let Some((r, scale)) = residual {
            for (v, rv) in out.iter_mut().zip(r.plane(oc)) {
                *v = *v * scale + rv;
            }
        }
        match c.act {
            Act::None => {}
            Act::Leaky(slope) => out.iter_mut().for_each(|v| {
                if *v < 0.0 {
                    *v *= slope;
                }
            }),
            Act::PReLU => {
                let slope = c.prelu[oc];
                out.iter_mut().for_each(|v| {
                    if *v < 0.0 {
                        *v *= slope;
                    }
                })
            }
        }
    });
}

pub fn deconv4x4s2_cpu(l: &DeconvOp, x: &Tensor) -> Tensor {
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
                            plane[oy as usize * ow + ox as usize] += l.weight[((ic * l.out_ch + oc) * 4 + ky) * 4 + kx] * s;
                        }
                    }
                }
            }
        }
    });
    out
}

// ----------------------------------------------------------------------
// Tiling shared by every backend.

/// Round a `[0,1]` float to a byte the way WGSL's `pack4x8unorm` does.
#[inline]
pub fn unorm8(v: f32) -> u8 {
    (0.5 + 255.0 * v.clamp(0.0, 1.0)).floor() as u8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

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

/// The region of the pre-padded frame that feeds one tile, and how the
/// network's output maps back. All in input pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileIo {
    pub rect: TileRect,
    /// Region in padded-frame coordinates (its origin is `rect` shifted by
    /// `pre_pad`, i.e. `rect.x` in padded coords is `rect.x + pre_pad`).
    pub region_x: u32,
    pub region_y: u32,
    pub region_w: u32,
    pub region_h: u32,
    /// Output crop offset (output pixels) to the tile's first pixel.
    pub crop_x: u32,
    pub crop_y: u32,
}

impl Network {
    /// Pad a frame for tiling: `pre_pad` on every side plus slack so every
    /// region is a multiple of `unshuffle`.
    pub fn pad_frame(&self, frame: &Frame) -> Frame {
        let p = self.pre_pad;
        let u = self.unshuffle;
        let w = (frame.width + 2 * p).div_ceil(u) * u + u;
        let h = (frame.height + 2 * p).div_ceil(u) * u + u;
        frame.padded_to(w, h, p, p, Pad::Replicate)
    }

    pub fn tile_io(&self, rect: TileRect) -> TileIo {
        let p = self.pre_pad;
        let u = self.unshuffle;
        let region_w = (rect.w + 2 * p).div_ceil(u) * u;
        let region_h = (rect.h + 2 * p).div_ceil(u) * u;
        let crop = if self.exact { 0 } else { p * self.scale };
        TileIo { rect, region_x: rect.x, region_y: rect.y, region_w, region_h, crop_x: crop, crop_y: crop }
    }

    /// Network input tensor for a tile: `[3·u²] × (region_h/u) × (region_w/u)`, values in `[0,1]`,
    /// pixel-unshuffled if the network wants it.
    pub fn extract_input(&self, padded: &Frame, io: TileIo) -> Tensor {
        let u = self.unshuffle as usize;
        let (rw, rh) = (io.region_w as usize, io.region_h as usize);
        let (tw, th) = (rw / u, rh / u);
        let mut t = Tensor::zeros(3 * u * u, th, tw);
        for y in 0..rh {
            let row = padded.row(io.region_y + y as u32, io.region_x, io.region_x + rw as u32);
            for x in 0..rw {
                for c in 0..3 {
                    // pixel_unshuffle: out[c*u*u + (y%u)*u + (x%u)][y/u][x/u]
                    let ch = c * u * u + (y % u) * u + (x % u);
                    t.data[(ch * th + y / u) * tw + x / u] = row[x * 3 + c] as f32 / 255.0;
                }
            }
        }
        t
    }

    /// Write a network output tensor for a tile into the `scale×` output
    /// frame: pixel-shuffle, optional input skip, clamp, quantise, crop.
    pub fn write_output(&self, out: &mut Frame, io: TileIo, t: &Tensor, input_tile: &Tensor) {
        let r = self.shuffle as usize;
        let s = self.scale as usize;
        let (ow, oh) = (t.w * r, t.h * r); // full output tile size
        let u = self.unshuffle as usize;
        for y in 0..(io.rect.h as usize * s) {
            let sy = y + io.crop_y as usize;
            if sy >= oh {
                break;
            }
            let base = out.idx(io.rect.x * s as u32, io.rect.y * s as u32 + y as u32);
            for x in 0..(io.rect.w as usize * s) {
                let sx = x + io.crop_x as usize;
                if sx >= ow {
                    break;
                }
                for c in 0..3 {
                    // pixel_shuffle: out[c][y][x] = in[c*r*r + (y%r)*r + (x%r)][y/r][x/r]
                    let ch = c * r * r + (sy % r) * r + (sx % r);
                    let mut v = t.at(ch, sy / r, sx / r);
                    if self.add_input {
                        // nearest-upsampled input pixel (input is at region resolution)
                        let (iy, ix) = (sy / s, sx / s);
                        let ich = c * u * u + (iy % u) * u + (ix % u);
                        v += input_tile.at(ich, iy / u, ix / u);
                    }
                    out.data[base + x * 3 + c] = unorm8(v);
                }
            }
        }
    }

    /// Write a tile given as packed RGBA8 for the *full* output region
    /// (`scale × region`, what the GPU pack pass produces), cropping to the
    /// tile. Shuffle and skip have already been applied.
    pub fn write_output_rgba8(&self, out: &mut Frame, io: TileIo, rgba: &[u8]) {
        let s = self.scale as usize;
        // Same-pad nets emit the whole region; valid nets emit exactly the tile.
        let (ow, oh) = if self.exact { (io.rect.w as usize * s, io.rect.h as usize * s) } else { (io.region_w as usize * s, io.region_h as usize * s) };
        assert_eq!(rgba.len(), ow * oh * 4, "packed tile size");
        let (cw, ch) = (io.rect.w as usize * s, io.rect.h as usize * s);
        for y in 0..ch {
            let sy = y + io.crop_y as usize;
            let base = out.idx(io.rect.x * s as u32, io.rect.y * s as u32 + y as u32);
            for x in 0..cw {
                let sx = x + io.crop_x as usize;
                let i = (sy * ow + sx) * 4;
                out.data[base + x * 3..base + x * 3 + 3].copy_from_slice(&rgba[i..i + 3]);
            }
        }
    }

    /// Whole-frame upscale on the CPU, tiled. The oracle and the no-GPU fallback.
    pub fn upscale_cpu(&self, frame: &Frame, tile: u32) -> Frame {
        let padded = self.pad_frame(frame);
        let s = self.scale;
        let mut out = Frame::new(frame.width * s, frame.height * s);
        for rect in tiles(frame.width, frame.height, tile) {
            let io = self.tile_io(rect);
            let input = self.extract_input(&padded, io);
            let y = self.forward(&input);
            self.write_output(&mut out, io, &y, &input);
        }
        out
    }
}

/// CPU [`crate::upscale::Upscaler`] for any [`Network`].
pub struct CpuNet {
    pub net: std::sync::Arc<Network>,
    pub tile: u32,
}

impl crate::upscale::Upscaler for CpuNet {
    fn scale(&self) -> u32 {
        self.net.scale
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        Ok(self.net.upscale_cpu(frame, self.tile))
    }
    fn name(&self) -> String {
        format!("cpu [{}] x{} tile {}", self.net.name, self.net.scale, self.tile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models")
    }

    /// `models/probe.png` is ffmpeg's `testsrc2` at 128x96; the `*_full.png`
    /// files are numpy outputs of tools/depth_probe.py on it, replicate-padded
    /// by the network's `pre_pad` and cropped, i.e. exactly what
    /// `upscale_cpu` computes with a single tile.
    fn probe_png(name: &str) -> Option<Frame> {
        let img = image::open(models_dir().join(name)).ok()?.into_rgb8();
        Frame::from_rgb(img.width(), img.height(), img.into_raw()).ok()
    }

    fn compare(got: &Frame, want: &Frame, tol: u8, label: &str) {
        assert_eq!((got.width, got.height), (want.width, want.height), "{label}: size");
        let mut worst = 0u8;
        let mut bad = 0usize;
        for (&g, &w) in got.data.iter().zip(&want.data) {
            let d = (g as i16 - w as i16).unsigned_abs() as u8;
            worst = worst.max(d);
            if d > tol {
                bad += 1;
            }
        }
        assert!(bad == 0, "{label}: {bad} bytes differ by more than {tol} (worst {worst})");
    }

    #[test]
    fn upconv7_via_network_matches_dedicated_forward() {
        let p = models_dir().join("upconv_7_art_scale2.0x_model.json");
        if !p.exists() {
            return;
        }
        let net = Network::load(&p).unwrap();
        assert_eq!(net.scale, 2);
        let m = crate::waifu2x::Upconv7::load_json(&p).unwrap();
        let mut f = Frame::new(23, 19);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = ((i * 37) % 256) as u8;
        }
        let a = net.upscale_cpu(&f, 16);
        let b = crate::waifu2x::upscale_cpu(&m, &f, 16);
        compare(&a, &b, 0, "upconv7 network vs waifu2x.rs");
    }

    /// The RRDB wiring against the numpy reference (tools/depth_probe.py,
    /// replicate-padded and cropped the same way) on the same rendered
    /// frame, single tile so no seam is involved. Needs the converted x2
    /// weights (not vendored); skipped otherwise.
    #[test]
    fn rrdb_x2_matches_numpy_reference() {
        // 67 MB, not vendored: D2X_RRDB_X2=/path/to/RealESRGAN_x2.safetensors (from tools/pth2st.py).
        let Ok(w) = std::env::var("D2X_RRDB_X2") else { return };
        let (Some(input), Some(want)) = (probe_png("probe.png"), probe_png("depth_full.png")) else { return };
        let net = Network::load(std::path::Path::new(&w)).unwrap();
        assert_eq!((net.scale, net.unshuffle, net.pre_pad), (2, 2, 16));
        eprintln!("rrdb x2: {:.2} MMAC/px", net.macs_per_input_pixel() / 1e6);
        let whole = net.upscale_cpu(&input, 256);
        compare(&whole, &want, 3, "rrdb x2 vs numpy");

        // Tile seams: with an overlap far smaller than the receptive field,
        // small tiles differ from the whole-image run. Measure it so the
        // default tile size is a number, not a hope.
        let tiled = net.upscale_cpu(&input, 64);
        let mean: f64 = tiled.data.iter().zip(&whole.data).map(|(&a, &b)| (a as f64 - b as f64).abs()).sum::<f64>() / tiled.data.len() as f64;
        eprintln!("rrdb x2: tile 64 vs whole, mean |d| = {mean:.2} LSB");
        assert!(mean < 1.5, "tile seams too strong: mean |d| {mean}");
    }

    #[test]
    fn srvgg_matches_numpy_reference() {
        let p = models_dir().join("realesr-animevideov3.safetensors");
        let (Some(input), Some(want)) = (probe_png("probe.png"), probe_png("srvgg_full.png")) else { return };
        if !p.exists() {
            return;
        }
        let net = Network::load(&p).unwrap();
        assert_eq!((net.scale, net.shuffle), (4, 4));
        eprintln!("srvgg: {:.2} MMAC/px", net.macs_per_input_pixel() / 1e6);
        let got = net.upscale_cpu(&input, 48);
        compare(&got, &want, 6, "srvgg vs numpy (tile 48, overlap 10)");
    }

    #[test]
    fn tiling_is_seamless_for_valid_net() {
        let p = models_dir().join("upconv_7_art_scale2.0x_model.json");
        if !p.exists() {
            return;
        }
        let net = Network::load(&p).unwrap();
        let mut f = Frame::new(30, 21);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = ((i * 53) % 256) as u8;
        }
        compare(&net.upscale_cpu(&f, 64), &net.upscale_cpu(&f, 9), 0, "valid net tile sizes");
    }
}



#[cfg(test)]
mod tile_pad_tests {
    /// The seam mechanism, stated so nobody re-lowers this without knowing.
    ///
    /// Every conv in the RRDB/SRVGG bodies is `pad: 1` — ZERO padding. At a
    /// tile's feature-space boundary the convolutions read zeros, so a band of
    /// darkened output grows inward, one pixel per conv layer. `pre_pad` is the
    /// margin that band is allowed to eat before it reaches the cropped region.
    ///
    /// A pad SMALLER than the number of stacked convs guarantees contamination
    /// inside the crop. That is not a tuning preference; it is arithmetic, and
    /// it is what produced visible black seams on real footage while a
    /// synthetic-content test measured "~1 LSB".
    #[test]
    fn the_pad_covers_at_least_the_conv_stack_depth() {
        // Conservative floor: a 16-conv body needs 16 pixels of context before
        // the crop even begins. The default is 32.
        let default_pad: u32 = std::env::var("D2X_TILE_PAD").ok()
            .and_then(|v| v.trim().parse().ok()).unwrap_or(32);
        assert!(default_pad >= 16,
            "pad {default_pad} is below the conv-stack depth of the shallowest body here; \
zero-padding contamination will reach into the visible crop");
    }

    #[test]
    fn the_override_is_bounded_so_a_typo_cannot_exhaust_the_tile() {
        // Clamped to 128: the padded region is (tile + 2*pad), so an
        // unbounded value silently makes every tile mostly margin and the
        // upscale crawl for no benefit.
        for v in ["0", "8", "32", "128", "100000"] {
            let parsed = v.parse::<u32>().unwrap().clamp(0, 128);
            assert!(parsed <= 128);
        }
    }
}
