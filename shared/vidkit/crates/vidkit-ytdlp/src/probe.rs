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
    /// Uploaded subtitles, by language.
    #[serde(default)]
    pub subtitles: std::collections::BTreeMap<String, Vec<SubTrack>>,
    /// YouTube's speech recognition, by language -- the original track and
    /// machine translations of it.
    #[serde(default)]
    pub automatic_captions: std::collections::BTreeMap<String, Vec<SubTrack>>,
}

/// One rendition of a subtitle track.
#[derive(Debug, Clone, Deserialize)]
pub struct SubTrack {
    pub ext: Option<String>,
    pub url: Option<String>,
    pub name: Option<String>,
}

/// A caption track to fetch instead of running whisper.
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub url: String,
    pub lang: String,
    /// Machine-generated. Worse punctuation, but timed to the speech, which
    /// is all chapters and search need.
    pub automatic: bool,
}

/// Choose captions for `lang` (default English). Pure.
///
/// Order: uploaded subtitles in the language; then the ORIGINAL automatic
/// track (`en-orig` in current yt-dlp) -- recognised from the speech; then
/// the plain `en` automatic track, which on a video spoken in another
/// language is a machine TRANSLATION, so it comes last. VTT only: that is
/// what the parser reads, and YouTube offers it for every track.
pub fn pick_captions(p: &Probe, lang: Option<&str>) -> Option<Caption> {
    let lang = lang.unwrap_or("en");
    let vtt = |tracks: &Vec<SubTrack>| -> Option<String> {
        tracks.iter().find(|t| t.ext.as_deref() == Some("vtt")).and_then(|t| t.url.clone())
    };
    let base = |k: &str| k.split(['-', '_']).next().unwrap_or(k).to_string();
    if let Some((k, t)) = p.subtitles.iter().find(|(k, t)| base(k) == lang && vtt(t).is_some()) {
        return Some(Caption { url: vtt(t)?, lang: k.clone(), automatic: false });
    }
    let orig = format!("{lang}-orig");
    for key in [orig.as_str(), lang] {
        if let Some(url) = p.automatic_captions.get(key).and_then(vtt) {
            return Some(Caption { url, lang: key.to_string(), automatic: true });
        }
    }
    None
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
    /// Does this format carry video?
    ///
    /// `"none"` is the extractor SAYING there is no video track, and an absent
    /// or null `vcodec` is it saying nothing at all. Those are different and
    /// treating them alike is what made every direct URL unplayable: real
    /// yt-dlp emits `"vcodec": null` for anything it has not probed, so an
    /// ordinary `.mp4` link was classified as audio-only and rejected before
    /// anything else got a look.
    ///
    /// An unreported codec is therefore MAYBE video, and the container settles
    /// it a moment later. [`codec_reported`] is how a caller tells the two
    /// apart.
    pub fn has_video(&self) -> bool {
        !matches!(self.vcodec.as_deref(), Some("none") | Some(""))
    }

    /// Did the extractor actually say what the video codec is?
    pub fn codec_reported(&self) -> bool {
        !matches!(self.vcodec.as_deref(), None | Some("") | Some("null"))
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

#[cfg(test)]
mod caption_tests {
    use super::*;

    #[test]
    fn captions_prefer_uploaded_then_original_speech_then_translation() {
        let j = r#"{"id":"v","formats":[],
          "subtitles":{"de":[{"ext":"vtt","url":"https://s/de"}],"en-GB":[{"ext":"srv3","url":"x"},{"ext":"vtt","url":"https://s/en"}]},
          "automatic_captions":{"en":[{"ext":"vtt","url":"https://a/en"}],"en-orig":[{"ext":"vtt","url":"https://a/orig"}]}}"#;
        let p: Probe = serde_json::from_str(j).unwrap();
        assert_eq!(pick_captions(&p, None), Some(Caption { url: "https://s/en".into(), lang: "en-GB".into(), automatic: false }));
        assert_eq!(pick_captions(&p, Some("de")).map(|c| c.url), Some("https://s/de".into()));
        let mut p2 = p.clone();
        p2.subtitles.clear();
        assert_eq!(pick_captions(&p2, None).map(|c| c.url), Some("https://a/orig".into()), "speech, not translation");
        p2.automatic_captions.remove("en-orig");
        assert_eq!(pick_captions(&p2, None).map(|c| (c.url, c.automatic)), Some(("https://a/en".into(), true)));
        assert_eq!(pick_captions(&p2, Some("ja")), None);
    }

    #[test]
    fn a_probe_without_caption_fields_still_parses() {
        let p: Probe = serde_json::from_str(r#"{"id":"v","formats":[]}"#).unwrap();
        assert!(p.subtitles.is_empty() && pick_captions(&p, None).is_none());
    }
}
