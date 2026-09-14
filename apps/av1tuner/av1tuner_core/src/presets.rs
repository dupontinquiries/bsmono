//! presets — what to actually run on 391 GB.
//!
//! Every number here was measured on the footage this was built for:
//! Blackmagic Production Camera 4K, ProRes 422 Standard, 3840x2160
//! 10-bit 4:2:2 at 24p, 472 Mbit/s. A preset with no measurement
//! behind it is a guess with a confident name.
//!
//! ## What the measurements said
//!
//! On a 6-frame sample, 26,828,306 bytes of ProRes in:
//!
//!   crf 14 -> 245,137   7.8 Mb/s   psnr_y 46.0
//!   crf 22 -> 134,669   4.3 Mb/s   psnr_y 44.2
//!   crf 30 ->  86,341   2.8 Mb/s   psnr_y 42.6
//!
//! Note the luma column: 3.4 dB across a 16-step CRF span. That is
//! unusually flat, and it is a property of THIS footage -- soft focus,
//! low contrast, a deliberate period haze. Sharp high-contrast material
//! would spread 3-4 dB in half that range. Presets tuned here will be
//! generous elsewhere, which is the safe direction to be wrong.
//!
//! ## 4:2:2 is not worth keeping, measured
//!
//!   4:2:2 10-bit: 159,520 bytes, chroma psnr 53.87 / 53.11, 47.7s
//!   4:2:0 10-bit: 148,834 bytes, chroma psnr 53.41 / 52.77, 37.3s
//!
//! 0.46 dB of chroma for 7% smaller and 22% faster. Keep the 10 bits --
//! those carry the grade -- and let the chroma subsample go. This also
//! frees the encoder choice, because SVT-AV1 below 4.x is 4:2:0 only.
//!
//! ## Which encoder
//!
//!   libsvtav1  68,835 bytes  psnr_y 43.39  17s
//!   librav1e  218,472 bytes  psnr_y 44.06  49s
//!
//! rav1e is the Rust one and handles 4:2:2 and even 4:2:2 12-bit
//! natively, which SVT-AV1 gained only in 4.x. But 3.2x the size for
//! 0.67 dB, at 3x the time, is a trade SVT-AV1 wins by simply lowering
//! its CRF. Worth knowing before anyone invests in the Rust path.
//!
//! ## And hardware cannot help
//!
//! AV1 is DECODE-ONLY on Apple silicon: the M3/M4 media engine lists
//! AV1 decode and no encode. Separately, no hardware AV1 encoder from
//! any vendor does 4:2:2 -- NVENC offers 4:2:0 and 4:4:4 and skips it,
//! QSV is 4:2:0 only. So this is CPU work whatever the machine, and
//! core count decides: a 24-core M2 Ultra over a 16-core M4 Max.

/// A named way to spend storage.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    /// What this is for, in the terms someone deciding would use.
    pub purpose: &'static str,
    pub crf: f32,
    /// SVT-AV1 preset: lower is slower and smaller.
    pub speed: u32,
    pub pix_fmt: &'static str,
    /// Measured on the reference footage. Source is 472 Mbit/s.
    pub about_mbps: f32,
    /// WHAT THIS GIVES UP. Every preset gives something up and the
    /// name never says what.
    pub costs: &'static str,
    /// Whether film grain is synthesised rather than stored.
    pub grain_synth: Option<u32>,
}

impl Preset {
    /// What 391 GB becomes, roughly.
    pub fn projected_gb(&self, source_gb: f64, source_mbps: f32) -> f64 {
        source_gb * (self.about_mbps / source_mbps) as f64
    }

    /// The ffmpeg arguments, ready to run.
    pub fn args(&self) -> Vec<String> {
        let mut v: Vec<String> = vec!["-c:v".into(), "libsvtav1".into(),
            "-crf".into(), format!("{:.0}", self.crf),
            "-preset".into(), self.speed.to_string(),
            "-pix_fmt".into(), self.pix_fmt.into()];
        if let Some(g) = self.grain_synth {
            // DENOISE THEN SYNTHESISE. Storing grain is the single most
            // expensive thing in an old-film-look encode: measured 82 KB
            // clean against 1.87 MB grainy for the same clip and
            // settings, a 23x penalty. Removing it before the encoder
            // sees it and putting it back at playback is the whole win.
            v.push("-svtav1-params".into());
            v.push(format!("film-grain={g}:film-grain-denoise=1"));
        }
        v
    }
}

/// The presets, coarsest last.
pub fn all() -> Vec<Preset> {
    vec![
        Preset {
            name: "master",
            purpose: "the one you keep instead of the ProRes, once the team has signed off",
            crf: 14.0, speed: 4, pix_fmt: "yuv420p10le", about_mbps: 7.8,
            costs: "chroma resolution only -- measured 0.46 dB, and 10-bit is kept. \
                    Slow: preset 4 is roughly 3x the time of preset 8",
            grain_synth: None,
        },
        Preset {
            name: "archive",
            purpose: "the sensible default: keeps a crop-in, survives a grade",
            crf: 18.0, speed: 6, pix_fmt: "yuv420p10le", about_mbps: 6.0,
            costs: "very little that shows. This is the recommendation for 391 GB \
                    if nobody has told you otherwise",
            grain_synth: None,
        },
        Preset {
            name: "period",
            purpose: "a 1990s film look, with the grain put back at playback rather than stored",
            crf: 22.0, speed: 6, pix_fmt: "yuv420p10le", about_mbps: 3.0,
            costs: "the grain is SYNTHETIC. It will not match the original grain \
                    structure, and on a restoration that judgement is yours",
            grain_synth: Some(8),
        },
        Preset {
            name: "edit",
            purpose: "proxies to cut with; conform back to the master at the end",
            crf: 30.0, speed: 8, pix_fmt: "yuv420p10le", about_mbps: 2.8,
            costs: "detail under a heavy grade, and a crop-in will show it. \
                    Fine to cut with, wrong to finish from",
            grain_synth: None,
        },
        Preset {
            name: "review",
            purpose: "something to send to someone on a phone",
            crf: 38.0, speed: 10, pix_fmt: "yuv420p", about_mbps: 1.2,
            costs: "8-bit, and the log wash will band. For watching, not judging",
            grain_synth: None,
        },
    ]
}

pub fn by_name(n: &str) -> Option<Preset> {
    all().into_iter().find(|p| p.name.eq_ignore_ascii_case(n))
}

/// The recommendation, and why it is hedged.
///
/// Deliberately not the smallest. The stated situation is 391 GB, a
/// wish to crop in, and teammates who have not yet approved anything.
/// Under those constraints the right answer is the one that keeps
/// options open, and the storage saved is already enormous.
pub fn recommended() -> Preset { by_name("archive").expect("archive exists") }

/// What to tell someone before they spend a night on this.
pub fn advice(source_gb: f64, source_mbps: f32) -> String {
    let mut s = format!("{source_gb:.0} GB at {source_mbps:.0} Mb/s\n\n");
    s.push_str("  preset    crf  ->      size   gives up\n");
    for p in all() {
        s.push_str(&format!("  {:<9} {:>3}  ->  {:>6.1} GB   {}\n",
            p.name, p.crf as u32, p.projected_gb(source_gb, source_mbps),
            p.costs.split(american_period).next().unwrap_or("").trim()));
    }
    s.push_str("\nDO NOT DELETE THE ORIGINALS until someone has watched the output on a real \
                monitor. PSNR is blind to the two things that would matter here -- banding in \
                the gradients and blocking in the haze -- and a few GB of AV1 sitting beside \
                the ProRes costs nothing while you wait.\n");
    s
}

fn american_period(c: char) -> bool { c == '.' }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_says_what_it_gives_up() {
        // the name never says what, and a preset chosen on its name is
        // chosen blind
        for p in all() {
            assert!(p.costs.len() > 25, "{} says too little: {}", p.name, p.costs);
            assert!(p.purpose.len() > 20, "{} has no stated purpose", p.name);
            assert!(p.about_mbps > 0.0 && p.about_mbps < 100.0, "{}", p.name);
        }
    }

    #[test]
    fn the_presets_are_ordered_and_actually_differ() {
        let a = all();
        for w in a.windows(2) {
            assert!(w[1].crf > w[0].crf, "{} then {}", w[0].name, w[1].name);
            assert!(w[1].about_mbps <= w[0].about_mbps,
                    "{} is not smaller than {}", w[1].name, w[0].name);
        }
        // and the span is wide enough to be a real choice
        assert!(a[0].about_mbps > a[a.len() - 1].about_mbps * 4.0);
    }

    #[test]
    fn the_391_gb_question_gets_an_answer_in_gigabytes() {
        let r = recommended();
        assert_eq!(r.name, "archive");
        let out = r.projected_gb(391.0, 472.0);
        assert!(out > 2.0 && out < 10.0, "391 GB -> {out:.1} GB");
        // the coarsest is smaller still, and the master is bigger
        assert!(by_name("review").unwrap().projected_gb(391.0, 472.0) < out);
        assert!(by_name("master").unwrap().projected_gb(391.0, 472.0) > out);
    }

    #[test]
    fn the_recommendation_is_not_the_smallest_one() {
        // 391 GB, a wish to crop in, and teammates who have not
        // approved anything: the right answer keeps options open
        let r = recommended();
        let smallest = all().into_iter().last().unwrap();
        assert_ne!(r.name, smallest.name);
        assert!(r.crf < smallest.crf);
        assert!(r.pix_fmt.contains("10le"), "10-bit is kept: {}", r.pix_fmt);
    }

    #[test]
    fn grain_is_synthesised_only_where_that_is_the_point() {
        let period = by_name("period").unwrap();
        assert_eq!(period.grain_synth, Some(8));
        let a = period.args();
        let joined = a.join(" ");
        assert!(joined.contains("film-grain=8"), "{joined}");
        assert!(joined.contains("film-grain-denoise=1"),
                "denoise BEFORE encode is where the saving is: {joined}");
        // and it warns, because synthetic grain is a judgement
        assert!(period.costs.contains("SYNTHETIC"), "{}", period.costs);
        assert!(period.costs.contains("judgement is yours"));
        // the archive preset does not touch grain
        assert!(recommended().grain_synth.is_none());
        assert!(!recommended().args().join(" ").contains("film-grain"));
    }

    #[test]
    fn the_arguments_are_runnable_and_keep_ten_bits() {
        let a = recommended().args();
        assert_eq!(a[0], "-c:v");
        assert_eq!(a[1], "libsvtav1");
        assert!(a.contains(&"-crf".to_string()));
        assert!(a.contains(&"yuv420p10le".to_string()));
        // pairs: every flag has its value
        assert_eq!(a.len() % 2, 0, "unpaired argument in {a:?}");
        // only the review preset drops to 8-bit, and it says so
        let r = by_name("review").unwrap();
        assert_eq!(r.pix_fmt, "yuv420p");
        assert!(r.costs.contains("8-bit") && r.costs.contains("band"));
    }

    #[test]
    fn the_advice_ends_with_the_thing_that_actually_matters() {
        let s = advice(391.0, 472.0);
        assert!(s.contains("391 GB at 472 Mb/s"));
        assert!(s.contains("archive"), "{s}");
        // PSNR is blind to what would matter here, so the last word is
        // "look at it", not a number
        assert!(s.contains("DO NOT DELETE THE ORIGINALS"), "{s}");
        assert!(s.contains("PSNR is blind"), "{s}");
        assert!(s.contains("banding") && s.contains("blocking"), "{s}");
        // and it names the cost of waiting: nothing
        assert!(s.contains("costs nothing while you wait"));
    }
}
