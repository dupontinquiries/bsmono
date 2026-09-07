//! The subset of `yt-dlp -J` we actually consume.
//!
//! Everything is `Option` because extractors are wildly inconsistent about
//! which fields they populate. A missing `fps` must degrade to a guess, never
//! to a parse failure — a hard-failing deserializer here means the player
//! breaks whenever YouTube ships a schema tweak.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Probe {
    pub id: Option<String>,
    pub title: Option<String>,
    pub duration: Option<f64>,
    pub is_live: Option<bool>,
    #[serde(default)]
    pub formats: Vec<RawFormat>,
    /// Present on progressive-only extractors that emit no `formats` array.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawFormat {
    pub format_id: String,
    pub url: Option<String>,
    pub ext: Option<String>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// Total bitrate, kbit/s.
    pub tbr: Option<f64>,
    /// Video bitrate, kbit/s.
    pub vbr: Option<f64>,
    /// Audio bitrate, kbit/s.
    pub abr: Option<f64>,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    /// "https", "m3u8_native", "http_dash_segments", ...
    pub protocol: Option<String>,
    /// "SDR", "HDR10", "HLG"...
    pub dynamic_range: Option<String>,
    pub language: Option<String>,
    /// **Must** be forwarded verbatim to whatever fetches the bytes. YouTube
    /// 403s on a mismatched User-Agent, and the signed URL is bound to them.
    #[serde(default)]
    pub http_headers: std::collections::BTreeMap<String, String>,
}

impl RawFormat {
    pub fn has_video(&self) -> bool {
        !matches!(self.vcodec.as_deref(), None | Some("none") | Some(""))
    }

    pub fn has_audio(&self) -> bool {
        !matches!(self.acodec.as_deref(), None | Some("none") | Some(""))
    }

    /// Bits per second, from whichever field the extractor bothered to fill.
    pub fn bitrate_bps(&self) -> Option<u64> {
        self.vbr
            .or(self.tbr)
            .map(|kbit| (kbit * 1000.0) as u64)
            .or_else(|| None)
    }

    /// yt-dlp omits `fps` constantly. 30 is the right guess for web video and
    /// guessing low is safer than guessing high: it makes us *more* likely to
    /// accept a stream a software decoder can handle, and the decoder's real
    /// timing comes from PTS anyway.
    pub fn fps_or_guess(&self) -> u32 {
        self.fps.map(|f| f.round() as u32).unwrap_or(30).max(1)
    }
}

pub fn parse(json: &str) -> Result<Probe, serde_json::Error> {
    serde_json::from_str(json)
}
