//! Which stream to actually fetch.
//!
//! This is where the caps model earns its keep. The same video, on the same
//! network, resolves to AV1 on an Asahi laptop and VP9 on a Fairphone —
//! because dav1d on Firestorm beats libavcodec's VP9, while Venus's
//! fixed-function VP9 beats dav1d on a phone by an order of magnitude in
//! power.
//!
//! Every candidate produces a [`Verdict`], kept and returned, so the UI can
//! show the full decision rather than an opaque answer.

use crate::probe::{Probe, RawFormat};
use std::cmp::Ordering;
use vidkit_core::caps::{Cost, DeviceCaps};
use vidkit_core::codec::{parse_codec_string, VideoProfile};

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Rejected(Reject),
    Scored(Score),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reject {
    /// The extractor did not say what the codec is. Recoverable: the container
    /// knows, and our own demuxer reads it. Kept distinct from
    /// `UnknownCodec`, which means it told us something we do not support.
    CodecNotReported,
    NotVideo,
    NoUrl,
    UnknownCodec(String),
    /// Nothing on this device can decode it at this size and rate.
    NoDecoder,
    /// Would blow the metered-connection ceiling.
    OverBitrateBudget { bps: u64, cap: u64 },
    /// HDR stream, SDR-only output. Tonemapping in the convert pass is
    /// possible but costs a pass and looks worse than just taking the SDR
    /// rendition, which YouTube always offers alongside.
    HdrOnSdrDisplay,
    /// The stream is a live HLS/DASH manifest and this path only does
    /// progressive. Segment handling is a separate source implementation.
    UnsupportedProtocol(String),
}

/// Ordered so that `max_by` picks the winner. Fields are compared in
/// declaration order by the manual `Ord` below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    /// Lower is better, so it is stored negated for max-ordering sanity.
    /// Kept as the raw `Cost` and inverted in `cmp`.
    pub cost: Cost,
    /// Pixels, clamped to the display. Higher is better, but a stream larger
    /// than the panel scores no higher than one that exactly fits — there is
    /// no point paying to decode pixels we will throw away.
    pub effective_pixels: u64,
    /// Pixels actually decoded, unclamped. Lower is better.
    ///
    /// Without this, clamping makes a 4K and a 1080p stream tie on
    /// `effective_pixels` for a 1080p panel, and the tie then breaks on frame
    /// rate — so the player decodes four times the pixels and discards three
    /// quarters of them to gain 60 fps. On a fanless laptop and a phone that
    /// is the wrong trade.
    ///
    /// The consequence, stated plainly: on a 1080p panel a 1080p30 stream beats
    /// a 4K60 one. If you would rather have the motion, move this comparison
    /// below `effective_fps`.
    pub source_pixels: u64,
    /// Capped at the display refresh.
    pub effective_fps: u32,
    pub bitrate_bps: u64,
    pub profile: VideoProfile,
}

impl Ord for Score {
    fn cmp(&self, other: &Self) -> Ordering {
        // Cost first and always. A hardware 1080p path beats a software 4K one
        // on every device we target; on a phone it is not close.
        other
            .cost
            .cmp(&self.cost) // reversed: lower Cost wins
            .then(self.effective_pixels.cmp(&other.effective_pixels))
            .then(other.source_pixels.cmp(&self.source_pixels))
            .then(self.effective_fps.cmp(&other.effective_fps))
            // Among otherwise-equal renditions prefer the *smaller* one: same
            // picture, less radio time, less battery.
            .then(other.bitrate_bps.cmp(&self.bitrate_bps))
    }
}

impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub format_id: String,
    pub verdict: Verdict,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub video_format_id: String,
    /// `None` when the chosen video format is muxed (already has audio).
    pub audio_format_id: Option<String>,
    pub score: Score,
    /// One line, for the UI: "VP9 1080p30 via Venus (fixed-function)".
    pub summary: String,
    /// Every candidate and why it won or lost.
    pub trace: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    NoFormats,
    /// Every format was rejected. The trace says why for each.
    NothingPlayable,
}

/// Progressive HTTP only. Segmented protocols need a different `MediaSource`.
fn protocol_ok(p: Option<&str>) -> bool {
    matches!(p, None | Some("https") | Some("http"))
}

fn is_hdr(f: &RawFormat) -> bool {
    !matches!(f.dynamic_range.as_deref(), None | Some("SDR") | Some(""))
}

fn evaluate(f: &RawFormat, caps: &DeviceCaps) -> Verdict {
    if !f.has_video() {
        return Verdict::Rejected(Reject::NotVideo);
    }
    if f.url.is_none() {
        return Verdict::Rejected(Reject::NoUrl);
    }
    if !protocol_ok(f.protocol.as_deref()) {
        return Verdict::Rejected(Reject::UnsupportedProtocol(
            f.protocol.clone().unwrap_or_default(),
        ));
    }

    let raw_vcodec = f.vcodec.clone().unwrap_or_default();
    let profile = match parse_codec_string(&raw_vcodec) {
        Some(p) => p,
        // An ABSENT or null vcodec is not the same as an unrecognised one, and
        // conflating them threw away a playable stream. Real yt-dlp reports
        // `"vcodec": null` for any URL it has not probed -- which is every
        // direct media link through the generic extractor -- and the first
        // time this ran against the real binary instead of JSON I had written,
        // every such stream was rejected as NoPlayableFormat.
        //
        // The container knows. Our own demuxer reads the codec from the file,
        // so refusing here on the strength of missing metadata is refusing to
        // look. It is accepted as a LAST RESORT below, because an unknown codec
        // cannot be scored against the device's decoders and preferring it over
        // a format we can actually reason about would be guessing.
        // ABSENT is not the same as unrecognised, and conflating them threw
        // away a playable stream. Real yt-dlp reports `"vcodec": null` for any
        // URL it has not probed -- every direct link through the generic
        // extractor -- and the first run against the real binary rather than
        // JSON I had written rejected all of them as NoPlayableFormat.
        None if raw_vcodec.is_empty() || raw_vcodec.eq_ignore_ascii_case("null") => {
            return Verdict::Rejected(Reject::CodecNotReported);
        }
        None => return Verdict::Rejected(Reject::UnknownCodec(raw_vcodec)),
    };

    let b = &caps.budget;
    if is_hdr(f) && !b.hdr_capable {
        return Verdict::Rejected(Reject::HdrOnSdrDisplay);
    }

    let w = f.width.unwrap_or(0);
    let h = f.height.unwrap_or(0);
    let fps = f.fps_or_guess();

    let path = match caps.best_path(&profile, w, h, fps) {
        Some(p) => p,
        None => return Verdict::Rejected(Reject::NoDecoder),
    };

    let bps = f.bitrate_bps().unwrap_or(0);
    if let Some(cap) = b.max_bitrate {
        if bps > cap {
            return Verdict::Rejected(Reject::OverBitrateBudget { bps, cap });
        }
    }

    // Clamp to the panel. A phone in portrait still plays landscape video, so
    // compare against the larger and smaller dimensions rather than assuming
    // an orientation.
    //
    // On battery this is the panel as the battery caps see it -- a smaller,
    // slower screen -- which is where `on_battery` finally does something. It
    // is applied here rather than as a scoring penalty so it composes with the
    // clamp instead of fighting it.
    let (dl, ds, max_hz) = b.effective_display();
    let (sl, ss) = (w.max(h), w.min(h));
    let effective_pixels = (sl.min(dl) as u64) * (ss.min(ds) as u64);

    Verdict::Scored(Score {
        cost: path.cost,
        effective_pixels,
        source_pixels: (w as u64) * (h as u64),
        effective_fps: fps.min(max_hz),
        bitrate_bps: bps,
        profile,
    })
}

/// Pick an audio-only track. Simple on purpose: prefer the requested language,
/// then Opus over AAC, then the highest bitrate under 160 kbit/s — above that
/// is inaudible on a phone speaker and costs radio time.
fn pick_audio<'a>(probe: &'a Probe, prefer_lang: Option<&str>) -> Option<&'a RawFormat> {
    probe
        .formats
        .iter()
        .filter(|f| f.has_audio() && !f.has_video() && f.url.is_some())
        .filter(|f| protocol_ok(f.protocol.as_deref()))
        .max_by_key(|f| {
            let lang_match = match (prefer_lang, f.language.as_deref()) {
                (Some(want), Some(have)) if want.eq_ignore_ascii_case(have) => 2,
                (Some(_), None) => 1,
                (None, _) => 1,
                _ => 0,
            };
            let opus = matches!(f.acodec.as_deref(), Some(a) if a.starts_with("opus")) as u8;
            let abr = f.abr.unwrap_or(0.0);
            // Prefer <=160, and among those the highest.
            let abr_score = if abr <= 160.0 {
                (abr * 10.0) as i64
            } else {
                // Over budget: rank below everything reasonable but above nothing.
                1_600 - (abr as i64)
            };
            (lang_match, opus, abr_score)
        })
}

pub fn select(
    probe: &Probe,
    caps: &DeviceCaps,
    prefer_lang: Option<&str>,
) -> Result<Selection, SelectError> {
    if probe.formats.is_empty() {
        return Err(SelectError::NoFormats);
    }

    let mut trace = Vec::with_capacity(probe.formats.len());
    let mut best: Option<(&RawFormat, Score)> = None;

    for f in &probe.formats {
        let verdict = evaluate(f, caps);
        if let Verdict::Scored(ref s) = verdict {
            let replace = match &best {
                None => true,
                Some((_, cur)) => s > cur,
            };
            if replace {
                best = Some((f, s.clone()));
            }
        }
        trace.push(Candidate {
            format_id: f.format_id.clone(),
            verdict,
        });
    }

    let (vf, score) = best.ok_or(SelectError::NothingPlayable)?;

    let audio = if vf.has_audio() {
        None
    } else {
        pick_audio(probe, prefer_lang).map(|a| a.format_id.clone())
    };

    let path = caps
        .best_path(
            &score.profile,
            vf.width.unwrap_or(0),
            vf.height.unwrap_or(0),
            vf.fps_or_guess(),
        )
        .expect("scored format must have a decode path");

    let summary = format!(
        "{} {}x{}@{} via {:?} ({:?}, {})",
        score.profile.codec,
        vf.width.unwrap_or(0),
        vf.height.unwrap_or(0),
        vf.fps_or_guess(),
        path.backend,
        path.cost,
        if path.delivery == vidkit_core::caps::Delivery::DmaBuf {
            "zero-copy"
        } else {
            "cpu upload"
        }
    );

    Ok(Selection {
        video_format_id: vf.format_id.clone(),
        audio_format_id: audio,
        score,
        summary,
        trace,
    })
}
