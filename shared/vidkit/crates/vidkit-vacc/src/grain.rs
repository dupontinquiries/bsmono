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

/// Grain parameters for every SHOWN frame of an AV1 IVF file, in display
/// order.
pub fn params_from_ivf(ivf: &[u8]) -> Result<Vec<Option<GrainParams>>, String> {
    let (_, frames) = crate::rap::ivf_frames(ivf).ok_or("not IVF")?;
    let mut parser = Av1Parser::new();
    let mut sps = None;
    let mut tracker = GrainTracker::default();
    let mut out = Vec::new();
    for tu in frames {
        let mut off = 0;
        while off < tu.len() {
            let Some((hdr, start, size)) = parser.read_obu(&tu[off..]).map_err(|e| format!("{e:?}"))? else { break };
            let data = &tu[off + start..(off + start + size).min(tu.len())];
            match hdr.obu_type {
                ObuType::SequenceHeader => sps = Some(parser.parse_sequence_header_obu(data).map_err(|e| format!("{e:?}"))?),
                ObuType::FrameHeader | ObuType::Frame => {
                    let s = sps.as_ref().ok_or("frame before sequence header")?;
                    let fh = parser.parse_frame_header(data, s, hdr.temporal_id, hdr.spatial_id).map_err(|e| format!("{e:?}"))?;
                    // The parser's per-slot order hints are updated only by
                    // commit_decoded(), which vacc's decode backends call
                    // after each frame. Used alone, the parser saw order hint
                    // 0 for every reference, judged skip mode not allowed,
                    // skipped the skip_mode_present bit, and misread every
                    // later field -- film_grain_params() is the last one
                    // (found by comparing with ffmpeg's trace_headers).
                    if !fh.show_existing_frame {
                        parser.dpb_mut().commit_decoded(0, &fh, s.order_hint_bits_minus1 as u32);
                    }
                    if let Some(p) = tracker.frame(&fh) {
                        out.push(p);
                    }
                }
                _ => {}
            }
            off += start + size;
        }
    }
    Ok(out)
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
        let params = params_from_ivf(&std::fs::read(&ivf).unwrap()).unwrap();
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
