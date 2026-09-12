//! Codec identity, parsed out of RFC 6381 codec strings.
//!
//! yt-dlp hands us strings like `av01.0.08M.10`, `vp09.02.10.10`, `vp9`,
//! `avc1.640028`, `hev1.2.4.L120.90`. Container-level probing hands us the
//! same shapes. One parser, used by both, so format selection and decoder
//! capability matching can never disagree about what a stream is.

use core::fmt;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Codec {
    H264,
    Hevc,
    Vp8,
    Vp9,
    Av1,
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Codec::H264 => "h264",
            Codec::Hevc => "hevc",
            Codec::Vp8 => "vp8",
            Codec::Vp9 => "vp9",
            Codec::Av1 => "av1",
        })
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Chroma {
    Yuv420,
    Yuv422,
    Yuv444,
}

/// Transfer function. Matters because it decides whether the convert pass
/// tonemaps or just does a matrix multiply.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Transfer {
    /// BT.709 / sRGB-ish. The overwhelming majority of streams.
    Sdr,
    /// SMPTE ST 2084.
    Pq,
    /// ARIB STD-B67.
    Hlg,
}

/// Everything a decoder needs to answer "can I take this?".
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct VideoProfile {
    pub codec: Codec,
    /// Codec-specific profile number. AV1 0/1/2, VP9 0/1/2/3, H.264 as in
    /// profile_idc, HEVC 1/2/3. `None` when the codec string was the bare
    /// short form (`"vp9"`) and carried no profile.
    pub profile: Option<u8>,
    pub bit_depth: u8,
    pub chroma: Chroma,
    /// Codec level x10 where the codec expresses one (H.264 4.1 -> 41).
    pub level: Option<u16>,
}

impl VideoProfile {
    pub fn is_10bit_or_deeper(&self) -> bool {
        self.bit_depth > 8
    }
}

/// Parse an RFC 6381 style codec string. Returns `None` for `"none"`, audio
/// codecs, and anything unrecognised — callers treat that as "not a video
/// stream I can play".
///
/// Deliberately lenient: real-world strings are truncated and malformed all
/// the time, and a stream we can decode is better than a parse error. Missing
/// fields fall back to the codec's most common configuration.
pub fn parse_codec_string(s: &str) -> Option<VideoProfile> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let mut parts = lower.split('.');
    let fourcc = parts.next()?;

    match fourcc {
        // av01.P.LLT.DD[.M.CCC.cp.tc.mc.F]
        //      profile, level+tier, depth, monochrome, chroma subsampling...
        "av01" => {
            let profile = parts.next().and_then(|p| p.parse::<u8>().ok());
            // "08M" = level 8, Main tier. Level here is seq_level_idx, not x10.
            let level = parts
                .next()
                .map(|p| p.trim_end_matches(['m', 'h']))
                .and_then(|p| p.parse::<u16>().ok());
            let bit_depth = parts.next().and_then(|p| p.parse::<u8>().ok()).unwrap_or(8);
            // Field 5 is monochrome, field 6 is subsampling as three digits.
            let chroma = {
                let _mono = parts.next();
                match parts.next() {
                    Some("100") => Chroma::Yuv444, // 4:4:4 -> subsampling_x=1? see note
                    Some("110") => Chroma::Yuv422,
                    Some("111") | None => Chroma::Yuv420,
                    Some(_) => Chroma::Yuv420,
                }
            };
            Some(VideoProfile {
                codec: Codec::Av1,
                profile,
                bit_depth,
                chroma,
                level,
            })
        }
        // vp09.PP.LL.DD[.CC...]
        "vp09" => {
            let profile = parts.next().and_then(|p| p.parse::<u8>().ok());
            let level = parts.next().and_then(|p| p.parse::<u16>().ok());
            let bit_depth = parts.next().and_then(|p| p.parse::<u8>().ok()).unwrap_or(8);
            // VP9 profiles 0/2 are 4:2:0, 1/3 allow 4:2:2 and 4:4:4.
            let chroma = Chroma::Yuv420;
            Some(VideoProfile {
                codec: Codec::Vp9,
                profile,
                bit_depth,
                chroma,
                level,
            })
        }
        // Bare short forms yt-dlp still emits for some extractors.
        "vp9" | "vp9.0" => Some(VideoProfile {
            codec: Codec::Vp9,
            profile: None,
            bit_depth: 8,
            chroma: Chroma::Yuv420,
            level: None,
        }),
        "vp8" | "vp08" => Some(VideoProfile {
            codec: Codec::Vp8,
            profile: None,
            bit_depth: 8,
            chroma: Chroma::Yuv420,
            level: None,
        }),
        "av1" => Some(VideoProfile {
            codec: Codec::Av1,
            profile: Some(0),
            bit_depth: 8,
            chroma: Chroma::Yuv420,
            level: None,
        }),
        // avc1.PPCCLL — six hex digits: profile_idc, constraint flags, level_idc.
        "avc1" | "avc3" | "h264" => {
            let hex = parts.next().unwrap_or("");
            let (profile, level) = if hex.len() >= 6 {
                let p = u8::from_str_radix(&hex[0..2], 16).ok();
                let l = u16::from_str_radix(&hex[4..6], 16).ok().map(|v| v * 10 / 10);
                (p, l)
            } else {
                (None, None)
            };
            // High 10 (110) and High 4:4:4 (244) are the deep/wide ones.
            let bit_depth = if profile == Some(110) || profile == Some(244) {
                10
            } else {
                8
            };
            let chroma = if profile == Some(244) {
                Chroma::Yuv444
            } else if profile == Some(122) {
                Chroma::Yuv422
            } else {
                Chroma::Yuv420
            };
            Some(VideoProfile {
                codec: Codec::H264,
                profile,
                bit_depth,
                chroma,
                level,
            })
        }
        // hev1.P.C.LLLL.CC / hvc1...
        "hev1" | "hvc1" | "h265" | "hevc" => {
            let profile = parts.next().and_then(|p| {
                // May be prefixed "A"/"B" for profile space.
                p.trim_start_matches(['a', 'b', 'c']).parse::<u8>().ok()
            });
            let _compat = parts.next();
            let level = parts
                .next()
                .and_then(|p| p.trim_start_matches('l').parse::<u16>().ok());
            // Main 10 is profile 2.
            let bit_depth = if profile == Some(2) { 10 } else { 8 };
            Some(VideoProfile {
                codec: Codec::Hevc,
                profile,
                bit_depth,
                chroma: Chroma::Yuv420,
                level,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn av1_main_8bit() {
        let p = parse_codec_string("av01.0.08M.08").unwrap();
        assert_eq!(p.codec, Codec::Av1);
        assert_eq!(p.profile, Some(0));
        assert_eq!(p.bit_depth, 8);
        assert_eq!(p.level, Some(8));
    }

    #[test]
    fn av1_hdr_10bit() {
        let p = parse_codec_string("av01.0.12M.10.0.110.09.16.09.0").unwrap();
        assert_eq!(p.codec, Codec::Av1);
        assert_eq!(p.bit_depth, 10);
        assert!(p.is_10bit_or_deeper());
    }

    #[test]
    fn vp9_long_and_short_agree_on_codec() {
        let long = parse_codec_string("vp09.00.50.08").unwrap();
        let short = parse_codec_string("vp9").unwrap();
        assert_eq!(long.codec, short.codec);
        assert_eq!(long.profile, Some(0));
        assert_eq!(long.bit_depth, 8);
        assert_eq!(short.profile, None);
    }

    #[test]
    fn vp9_profile2_10bit() {
        let p = parse_codec_string("vp09.02.10.10.01.09.16.09.00").unwrap();
        assert_eq!(p.bit_depth, 10);
        assert_eq!(p.profile, Some(2));
    }

    #[test]
    fn h264_high_level_41() {
        let p = parse_codec_string("avc1.640029").unwrap();
        assert_eq!(p.codec, Codec::H264);
        assert_eq!(p.profile, Some(100)); // 0x64 = High
        assert_eq!(p.level, Some(41)); // 0x29 = 41
        assert_eq!(p.bit_depth, 8);
    }

    #[test]
    fn hevc_main10() {
        let p = parse_codec_string("hev1.2.4.L120.90").unwrap();
        assert_eq!(p.codec, Codec::Hevc);
        assert_eq!(p.bit_depth, 10);
    }

    #[test]
    fn none_and_audio_reject() {
        assert!(parse_codec_string("none").is_none());
        assert!(parse_codec_string("").is_none());
        assert!(parse_codec_string("opus").is_none());
        assert!(parse_codec_string("mp4a.40.2").is_none());
    }

    #[test]
    fn truncated_strings_do_not_panic() {
        for s in ["av01", "av01.0", "vp09.00", "avc1.64", "hev1.2"] {
            let _ = parse_codec_string(s);
        }
    }

    /// A missing depth field must default to 8, not 10. Defaulting high makes
    /// the capability check reject the stream on 8-bit-only hardware — the
    /// Fairphone's H.264 path, for one — over a field the extractor simply
    /// did not bother to emit.
    #[test]
    fn a_missing_bit_depth_defaults_to_eight() {
        assert_eq!(parse_codec_string("av01.0.08M").unwrap().bit_depth, 8);
        assert_eq!(parse_codec_string("vp09.00.50").unwrap().bit_depth, 8);
        assert!(!parse_codec_string("av01.0.08M").unwrap().is_10bit_or_deeper());
    }
}
