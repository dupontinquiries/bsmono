//! AV1 film grain parameters per shown frame, from the frame headers.
//!
//! The player's ffmpeg pipe carries pixels only, so a GPU grain stage needs
//! the parameters from the bitstream itself. vacc's parser reads
//! film_grain_params() but leaves `update_grain = 0` unresolved (hardware
//! decoders take the reference index); a software grain stage must do the
//! spec's load_grain_params(): keep each reference slot's parameters (per
//! refresh_frame_flags), give a non-updating frame its reference's
//! parameters with its own seed, and give show_existing_frame the shown
//! slot's.

use vacc_parser::av1::{Av1FilmGrain, Av1Parser, ObuType};
use vidkit_core::av1_grain::GrainParams;

/// Derived parameters from vacc's raw bitstream fields.
pub fn derive(g: &Av1FilmGrain) -> GrainParams {
    let pts = |v: &[u8], s: &[u8], n: u8| (0..n as usize).map(|i| [v[i], s[i]]).collect::<Vec<_>>();
    let raw = |c: i8| (c as u8 as i32 - 128) as i8;
    let mut uv = [[0i8; 25]; 2];
    for i in 0..25 {
        uv[0][i] = raw(g.ar_coeffs_cb_plus_128[i]);
        uv[1][i] = raw(g.ar_coeffs_cr_plus_128[i]);
    }
    let mut y = [0i8; 24];
    for (i, c) in y.iter_mut().enumerate() {
        *c = raw(g.ar_coeffs_y_plus_128[i]);
    }
    GrainParams {
        seed: g.grain_seed,
        y_points: pts(&g.point_y_value, &g.point_y_scaling, g.num_y_points),
        cb_points: pts(&g.point_cb_value, &g.point_cb_scaling, g.num_cb_points),
        cr_points: pts(&g.point_cr_value, &g.point_cr_scaling, g.num_cr_points),
        chroma_scaling_from_luma: g.chroma_scaling_from_luma,
        scaling_shift: g.grain_scaling_minus_8 + 8,
        ar_coeff_lag: g.ar_coeff_lag,
        ar_coeffs_y: y,
        ar_coeffs_uv: uv,
        ar_coeff_shift: g.ar_coeff_shift_minus_6 + 6,
        grain_scale_shift: g.grain_scale_shift,
        uv_mult: [g.cb_mult as i32 - 128, g.cr_mult as i32 - 128],
        uv_luma_mult: [g.cb_luma_mult as i32 - 128, g.cr_luma_mult as i32 - 128],
        uv_offset: [g.cb_offset as i32 - 256, g.cr_offset as i32 - 256],
        overlap: g.overlap_flag,
        clip_to_restricted_range: g.clip_to_restricted_range,
    }
}

/// Reference-slot tracking of grain parameters (spec load_grain_params).
#[derive(Default)]
pub struct GrainTracker {
    slots: [Option<Av1FilmGrain>; 8],
}

impl GrainTracker {
    /// One frame header in; the parameters to apply to it if it is shown
    /// (`None`: no grain), or `None` overall for a frame that is not shown.
    pub fn frame(&mut self, fh: &vacc_parser::av1::Av1FrameHeader) -> Option<Option<GrainParams>> {
        if fh.show_existing_frame {
            let g = self.slots[fh.frame_to_show_map_idx as usize & 7].clone();
            return Some(g.filter(|g| g.apply_grain).map(|g| derive(&g)));
        }
        let mut g = fh.film_grain.clone();
        if g.apply_grain && !g.update_grain {
            // The reference's parameters, this frame's seed.
            let seed = g.grain_seed;
            if let Some(r) = self.slots[g.film_grain_params_ref_idx as usize & 7].clone() {
                g = r;
                g.grain_seed = seed;
                g.update_grain = false;
            }
        }
        for i in 0..8 {
            if fh.refresh_frame_flags & (1 << i) != 0 {
                self.slots[i] = Some(g.clone());
            }
        }
        fh.show_frame.then(|| g.apply_grain.then(|| derive(&g)))
    }
}

/// Streaming form of [`params_from_ivf`]: one temporal unit at a time, as a
/// demuxer delivers them. Holds the parser state (sequence header, reference
/// order hints) and the grain reference slots across units.
pub struct GrainStream {
    parser: Av1Parser,
    sps: Option<vacc_core::picture::Av1Sps>,
    tracker: GrainTracker,
    /// Whether the sequence header says grain can appear at all; `None`
    /// before the first sequence header.
    pub present: Option<bool>,
}

impl Default for GrainStream {
    fn default() -> Self {
        Self { parser: Av1Parser::new(), sps: None, tracker: GrainTracker::default(), present: None }
    }
}

impl GrainStream {
    /// One temporal unit in. `Some(p)` for the unit's shown frame (`p` is
    /// `None` for a frame without grain); `None` if the unit shows nothing.
    pub fn push_tu(&mut self, tu: &[u8]) -> Result<Option<Option<GrainParams>>, String> {
        let mut shown = None;
        let mut off = 0;
        while off < tu.len() {
            let Some((hdr, start, size)) = self.parser.read_obu(&tu[off..]).map_err(|e| format!("{e:?}"))? else { break };
            let data = &tu[off + start..(off + start + size).min(tu.len())];
            match hdr.obu_type {
                ObuType::SequenceHeader => {
                    let s = self.parser.parse_sequence_header_obu(data).map_err(|e| format!("{e:?}"))?;
                    self.present = Some(s.film_grain_params_present);
                    self.sps = Some(s);
                }
                ObuType::FrameHeader | ObuType::Frame => {
                    let s = self.sps.as_ref().ok_or("frame before sequence header")?;
                    let fh = self.parser.parse_frame_header(data, s, hdr.temporal_id, hdr.spatial_id).map_err(|e| format!("{e:?}"))?;
                    // The parser's per-slot order hints are updated only by
                    // commit_decoded(), which vacc's decode backends call
                    // after each frame. Used alone, the parser saw order hint
                    // 0 for every reference, judged skip mode not allowed,
                    // skipped the skip_mode_present bit, and misread every
                    // later field -- film_grain_params() is the last one
                    // (found by comparing with ffmpeg's trace_headers).
                    if !fh.show_existing_frame {
                        self.parser.dpb_mut().commit_decoded(0, &fh, s.order_hint_bits_minus1 as u32);
                    }
                    if let Some(p) = self.tracker.frame(&fh) {
                        shown = Some(p);
                    }
                }
                _ => {}
            }
            off += start + size;
        }
        Ok(shown)
    }
}

/// Grain parameters for every SHOWN frame of an AV1 IVF file, in display
/// order.
pub fn params_from_ivf(ivf: &[u8]) -> Result<Vec<Option<GrainParams>>, String> {
    let (_, frames) = crate::rap::ivf_frames(ivf).ok_or("not IVF")?;
    let mut st = GrainStream::default();
    let mut out = Vec::new();
    for tu in frames {
        if let Some(p) = st.push_tu(tu)? {
            out.push(p);
        }
    }
    Ok(out)
}

/// An IVF stream read as it arrives (a pipe from `ffmpeg -c copy -f ivf`):
/// the header's time base, then (pts in seconds, temporal unit) per frame.
pub struct IvfReader<R> {
    r: R,
    /// Seconds per pts tick.
    pub tick: f64,
}

impl<R: std::io::Read> IvfReader<R> {
    pub fn new(mut r: R) -> Result<Self, String> {
        let mut h = [0u8; 32];
        r.read_exact(&mut h).map_err(|e| format!("IVF header: {e}"))?;
        if &h[..4] != b"DKIF" {
            return Err("not IVF".into());
        }
        let hlen = u16::from_le_bytes([h[6], h[7]]) as usize;
        if hlen > 32 {
            std::io::copy(&mut std::io::Read::take(r.by_ref(), (hlen - 32) as u64), &mut std::io::sink()).map_err(|e| e.to_string())?;
        }
        // Header bytes 16..24: time base as rate (denominator) then scale.
        let rate = u32::from_le_bytes([h[16], h[17], h[18], h[19]]).max(1);
        let scale = u32::from_le_bytes([h[20], h[21], h[22], h[23]]).max(1);
        Ok(Self { r, tick: scale as f64 / rate as f64 })
    }

    /// The next (pts seconds, unit); `None` at the end.
    pub fn next_unit(&mut self, buf: &mut Vec<u8>) -> Option<f64> {
        let mut fh = [0u8; 12];
        self.r.read_exact(&mut fh).ok()?;
        let n = u32::from_le_bytes([fh[0], fh[1], fh[2], fh[3]]) as usize;
        let pts = u64::from_le_bytes(fh[4..12].try_into().ok()?);
        buf.resize(n, 0);
        self.r.read_exact(buf).ok()?;
        Some(pts as f64 * self.tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn have(bin: &str) -> bool {
        Command::new(bin).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
    }

    /// The oracle: dav1d decodes a film-grain SVT-AV1 clip with grain off;
    /// our CPU grain, with parameters parsed from the headers, must give
    /// exactly dav1d's grained output -- every byte of every frame.
    #[test]
    fn cpu_grain_reproduces_dav1d_exactly() {
        if !have("dav1d") || !have("ffmpeg") {
            eprintln!("needs dav1d and ffmpeg; skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("vk-grain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (ivf, ng, gr) = (dir.join("c.ivf"), dir.join("ng.yuv"), dir.join("gr.yuv"));
        let (w, h) = (352usize, 198usize); // not a multiple of 32: partial blocks at the edges
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc2=size={w}x{h}:rate=30:duration=0.5,noise=alls=14:allf=t"),
            "-c:v", "libsvtav1", "-preset", "10", "-svtav1-params", "film-grain=12", "-pix_fmt", "yuv420p"]).arg(&ivf).stderr(Stdio::null()).status().unwrap().success());
        for (out, fg) in [(&ng, "0"), (&gr, "1")] {
            assert!(Command::new("dav1d").args(["-q", "--threads", "1", "--muxer", "yuv", "--filmgrain", fg, "-i"]).arg(&ivf).arg("-o").arg(out).status().unwrap().success());
        }
        let bytes = std::fs::read(&ivf).unwrap();
        let params = params_from_ivf(&bytes).unwrap();
        // The streaming reader (the player's path) agrees with the whole-file
        // one, and the IVF clock is 30 fps from zero.
        let mut rd = IvfReader::new(&bytes[..]).unwrap();
        let (mut st, mut buf, mut streamed, mut times) = (GrainStream::default(), Vec::new(), Vec::new(), Vec::new());
        while let Some(t) = rd.next_unit(&mut buf) {
            if let Some(p) = st.push_tu(&buf).unwrap() {
                streamed.push(p);
                times.push(t);
            }
        }
        assert_eq!(st.present, Some(true));
        assert_eq!(streamed, params);
        assert!(times.iter().enumerate().all(|(i, t)| (t * 30.0 - i as f64).abs() < 0.05), "{times:?}");
        let (ng, gr) = (std::fs::read(&ng).unwrap(), std::fs::read(&gr).unwrap());
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let fs = w * h + 2 * cw * ch;
        assert_eq!(ng.len() / fs, params.len(), "one parameter set per shown frame");
        let mut grained = 0;
        for (i, p) in params.iter().enumerate() {
            let mut f = ng[i * fs..(i + 1) * fs].to_vec();
            if let Some(p) = p {
                let (y, uv) = f.split_at_mut(w * h);
                let (u, v) = uv.split_at_mut(cw * ch);
                vidkit_core::av1_grain::apply_i420(p, y, u, v, w, h, false);
                grained += 1;
            }
            let want = &gr[i * fs..(i + 1) * fs];
            let bad = f.iter().zip(want).filter(|(a, b)| a != b).count();
            assert_eq!(bad, 0, "frame {i}: {bad} bytes differ from dav1d (seed {:?})", p.as_ref().map(|p| p.seed));
        }
        assert!(grained > 0, "the clip had no grain to test");
        eprintln!("DIAG {} frames, {grained} grained, byte-identical to dav1d", params.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
