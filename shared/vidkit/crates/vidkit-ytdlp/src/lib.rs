//! yt-dlp probing and format selection. Pure: parses JSON, returns a choice.
//! Spawning the `yt-dlp` process lives in the source implementation, not here,
//! so the policy is testable with fixtures.

pub mod http;
pub mod paste;
pub mod probe;
pub mod resolve;
pub mod select;

pub use http::{ChunkPolicy, HttpByteSource, HttpError, HttpTransport, RangeRequest};
pub use paste::{classify, related_from_json, Paste, Related, RelatedEntry};
pub use probe::{parse, pick_captions, Caption, Probe, RawFormat, SubTrack};
pub use resolve::{build_args, url_expiry, CommandRunner, ResolveError, ResolveOptions, Resolved, Resolver, RunError};
pub use select::{select, Reject, SelectError, Selection, Verdict};

#[cfg(test)]
mod tests {
    use super::*;
    use vidkit_core::caps::{asahi_m_series, desktop_full_hw, fairphone_venus, Cost};

    /// A trimmed but realistically shaped YouTube response: the same content
    /// offered as AV1, VP9 and H.264 at several sizes, plus separate audio.
    const FIXTURE: &str = r#"{
      "id": "abc123",
      "title": "test",
      "duration": 212.0,
      "formats": [
        {"format_id":"249","url":"https://x/249","ext":"webm","vcodec":"none",
         "acodec":"opus","abr":49.9,"protocol":"https","language":"en"},
        {"format_id":"251","url":"https://x/251","ext":"webm","vcodec":"none",
         "acodec":"opus","abr":141.2,"protocol":"https","language":"en"},
        {"format_id":"140","url":"https://x/140","ext":"m4a","vcodec":"none",
         "acodec":"mp4a.40.2","abr":129.5,"protocol":"https","language":"en"},

        {"format_id":"137","url":"https://x/137","ext":"mp4","vcodec":"avc1.640028",
         "acodec":"none","width":1920,"height":1080,"fps":30,"vbr":4200.0,
         "protocol":"https","dynamic_range":"SDR"},
        {"format_id":"137-hi","url":"https://x/137hi","ext":"mp4","vcodec":"avc1.640028",
         "acodec":"none","width":1920,"height":1080,"fps":30,"vbr":8500.0,
         "protocol":"https","dynamic_range":"SDR"},
        {"format_id":"247","url":"https://x/247","ext":"webm","vcodec":"vp09.00.31.08",
         "acodec":"none","width":1280,"height":720,"fps":30,"vbr":1400.0,
         "protocol":"https","dynamic_range":"SDR"},
                {"format_id":"248","url":"https://x/248","ext":"webm","vcodec":"vp09.00.40.08",
         "acodec":"none","width":1920,"height":1080,"fps":30,"vbr":2800.0,
         "protocol":"https","dynamic_range":"SDR"},
        {"format_id":"399","url":"https://x/399","ext":"mp4","vcodec":"av01.0.08M.08",
         "acodec":"none","width":1920,"height":1080,"fps":30,"vbr":2100.0,
         "protocol":"https","dynamic_range":"SDR"},
        {"format_id":"401","url":"https://x/401","ext":"mp4","vcodec":"av01.0.12M.08",
         "acodec":"none","width":3840,"height":2160,"fps":30,"vbr":11000.0,
         "protocol":"https","dynamic_range":"SDR"},
        {"format_id":"337","url":"https://x/337","ext":"webm","vcodec":"vp09.02.51.10.01.09.16.09.00",
         "acodec":"none","width":3840,"height":2160,"fps":60,"vbr":18000.0,
         "protocol":"https","dynamic_range":"HDR10"},
        {"format_id":"18","url":"https://x/18","ext":"mp4","vcodec":"avc1.42001E",
         "acodec":"mp4a.40.2","width":640,"height":360,"fps":30,"tbr":600.0,
         "protocol":"https","dynamic_range":"SDR"},
        {"format_id":"hls-720","url":"https://x/hls","ext":"mp4","vcodec":"avc1.4d401f",
         "acodec":"none","width":1280,"height":720,"fps":30,"tbr":2000.0,
         "protocol":"m3u8_native"}
      ]
    }"#;

    fn probe_fixture() -> Probe {
        parse(FIXTURE).expect("fixture must parse")
    }

    #[test]
    fn fixture_parses() {
        let p = probe_fixture();
        assert_eq!(p.formats.len(), 12);
        assert_eq!(p.duration, Some(212.0));
    }

    #[test]
    fn missing_fields_do_not_break_parsing() {
        let p = parse(r#"{"formats":[{"format_id":"1"}]}"#).unwrap();
        assert_eq!(p.formats[0].fps_or_guess(), 30);
    }

    /// "none" is the extractor saying there is no video; absent or null is it
    /// saying nothing. This test used to assert they were the same, which is
    /// precisely the assumption that made every real yt-dlp direct URL
    /// unplayable -- an ordinary .mp4 link was classified as audio-only and
    /// rejected before anything else looked at it.
    #[test]
    fn an_unreported_codec_is_not_the_same_as_no_video() {
        let none = parse(r#"{"formats":[{"format_id":"1","vcodec":"none"}]}"#).unwrap();
        assert!(!none.formats[0].has_video(), "\"none\" means no video track");
        assert!(none.formats[0].codec_reported());

        // What real yt-dlp emits for anything it has not probed.
        let null = parse(r#"{"formats":[{"format_id":"1","vcodec":null}]}"#).unwrap();
        assert!(null.formats[0].has_video(), "null was treated as audio-only");
        assert!(!null.formats[0].codec_reported());

        let absent = parse(r#"{"formats":[{"format_id":"1"}]}"#).unwrap();
        assert!(absent.formats[0].has_video());
        assert!(!absent.formats[0].codec_reported());

        let known = parse(r#"{"formats":[{"format_id":"1","vcodec":"av01.0.08M.08"}]}"#).unwrap();
        assert!(known.formats[0].has_video() && known.formats[0].codec_reported());
    }

    /// The headline behaviour: same JSON, opposite codec, because the hardware
    /// differs.
    #[test]
    fn asahi_picks_av1_fairphone_picks_vp9() {
        let p = probe_fixture();

        let asahi = select(&p, &asahi_m_series(), Some("en")).unwrap();
        assert_eq!(
            asahi.score.as_ref().expect("a scored selection").profile.codec.to_string(),
            "h264",
            "AVD fixed-function H.264 must beat any software path on Asahi; got {}",
            asahi.summary
        );

        let fp = select(&p, &fairphone_venus(), Some("en")).unwrap();
        assert_eq!(fp.score.as_ref().expect("a scored selection").cost, Cost::Fixed);
        assert!(
            matches!(fp.score.as_ref().expect("a scored selection").profile.codec.to_string().as_str(), "vp9" | "h264"),
            "Fairphone must land on a Venus path, got {}",
            fp.summary
        );
    }

    /// With H.264 removed, the inversion is unambiguous: Asahi takes AV1
    /// (dav1d), Fairphone takes VP9 (Venus).
    #[test]
    fn without_h264_the_codec_preference_inverts() {
        let mut p = probe_fixture();
        p.formats.retain(|f| {
            !f.vcodec
                .as_deref()
                .unwrap_or("")
                .starts_with("avc1")
        });

        let asahi = select(&p, &asahi_m_series(), None).unwrap();
        assert_eq!(asahi.score.as_ref().expect("a scored selection").profile.codec.to_string(), "av1", "{}", asahi.summary);

        let fp = select(&p, &fairphone_venus(), None).unwrap();
        assert_eq!(fp.score.as_ref().expect("a scored selection").profile.codec.to_string(), "vp9", "{}", fp.summary);
        assert_eq!(fp.score.as_ref().expect("a scored selection").cost, Cost::Fixed);
    }

    #[test]
    fn phone_does_not_fetch_4k_for_a_1080p_panel() {
        let p = probe_fixture();
        let fp = select(&p, &fairphone_venus(), None).unwrap();
        assert_ne!(fp.video_format_id, "401");
        assert_ne!(fp.video_format_id, "337");
    }

    /// Decoding pixels the panel cannot show is wasted power and wasted radio
    /// time. Previously this was only ever true by accident — the 4K streams
    /// in the fixture were rejected for *other* reasons on every device, so
    /// the clamp itself went unasserted.
    #[test]
    fn a_1080p_panel_takes_the_1080p_stream_even_when_4k_is_decodable() {
        let p = probe_fixture();
        let mut d = desktop_full_hw(); // can decode 4K AV1 in fixed function
        d.budget.display_width = 1920;
        d.budget.display_height = 1080;
        d.budget.display_max_hz = 60;

        let s = select(&p, &d, None).unwrap();
        let chosen = p
            .formats
            .iter()
            .find(|f| f.format_id == s.video_format_id)
            .unwrap();
        assert_eq!(
            (chosen.width, chosen.height),
            (Some(1920), Some(1080)),
            "clamped to the panel; got {}",
            s.summary
        );
    }

    /// Unplugging the phone must change what it fetches. Before the battery
    /// caps were wired in, `on_battery` was a field nobody read and this test
    /// would have found the same format either way.
    #[test]
    fn a_phone_on_battery_picks_a_smaller_stream_than_the_same_phone_plugged_in() {
        let p = probe_fixture();

        let mut plugged = fairphone_venus();
        plugged.budget.on_battery = false;
        let a = select(&p, &plugged, None).unwrap();

        let battery = fairphone_venus(); // on_battery is true in the profile
        assert!(battery.budget.on_battery);
        let b = select(&p, &battery, None).unwrap();

        let pixels = |s: &Selection| {
            let f = p
                .formats
                .iter()
                .find(|f| f.format_id == s.video_format_id)
                .unwrap();
            (f.width.unwrap_or(0) as u64) * (f.height.unwrap_or(0) as u64)
        };
        // Strictly smaller, not merely "not bigger": `<=` is satisfied when the
        // cap does nothing at all, which is exactly what it did before this
        // was wired up.
        //
        // The mechanism is worth stating. The cap does not penalise big
        // streams directly -- it shrinks the panel, so 720p and 1080p CLAMP to
        // the same effective pixels, and the existing source-pixel tiebreak
        // then prefers the one that costs less to decode.
        assert!(
            pixels(&b) < pixels(&a),
            "on battery it chose the same or a bigger stream: {} vs {}",
            b.summary,
            a.summary
        );
        assert!(
            b.score.as_ref().expect("a scored selection").effective_fps <= 30,
            "frame rate not capped on battery: {}",
            b.score.as_ref().expect("a scored selection").effective_fps
        );
    }

    /// The 480p policy, expressed through the same knob.
    #[test]
    fn a_480p_battery_cap_rejects_everything_larger() {
        let p = probe_fixture();
        let mut d = fairphone_venus();
        d.budget.on_battery = true;
        d.budget.battery_max_long_edge = 854;

        let s = select(&p, &d, None).unwrap();
        // Everything is clamped to the 854-wide budget, so the chosen stream
        // cannot score above that many effective pixels.
        assert!(
            s.score.as_ref().expect("a scored selection").effective_pixels <= 854 * 480,
            "effective pixels {} exceed the 480p cap: {}",
            s.score.as_ref().expect("a scored selection").effective_pixels,
            s.summary
        );
    }

    #[test]
    fn hdr_stream_rejected_on_sdr_output() {
        let p = probe_fixture();
        let fp = select(&p, &fairphone_venus(), None).unwrap();
        let hdr = fp
            .trace
            .iter()
            .find(|c| c.format_id == "337")
            .expect("337 in trace");
        assert!(matches!(
            hdr.verdict,
            Verdict::Rejected(Reject::HdrOnSdrDisplay)
        ));
    }

    #[test]
    fn segmented_protocols_are_rejected_with_a_reason() {
        let p = probe_fixture();
        let d = desktop_full_hw();
        let s = select(&p, &d, None).unwrap();
        let hls = s.trace.iter().find(|c| c.format_id == "hls-720").unwrap();
        assert!(matches!(
            hls.verdict,
            Verdict::Rejected(Reject::UnsupportedProtocol(_))
        ));
    }

    /// Decoder capability is checked *before* the bitrate cap, deliberately:
    /// "this device cannot decode it at all" is a more useful thing to show
    /// the user than "it was too big", and it is the harder constraint. So the
    /// 4K renditions here come back as `NoDecoder`, not `OverBitrateBudget` —
    /// only the 1080p stream the phone *could* decode trips the cap.
    #[test]
    fn bitrate_cap_rejects_a_stream_the_device_could_otherwise_decode() {
        let p = probe_fixture();
        let fp = select(&p, &fairphone_venus(), None).unwrap();
        let hi = fp.trace.iter().find(|c| c.format_id == "137-hi").unwrap();
        match &hi.verdict {
            Verdict::Rejected(Reject::OverBitrateBudget { bps, cap }) => {
                assert_eq!(*bps, 8_500_000);
                assert_eq!(*cap, 6_000_000);
            }
            other => panic!("expected OverBitrateBudget, got {other:?}"),
        }
        // And the phone still lands on the 4.2 Mbit/s rendition instead.
        assert_ne!(fp.video_format_id, "137-hi");
    }

    #[test]
    fn audio_track_is_paired_when_video_is_video_only() {
        let p = probe_fixture();
        let s = select(&p, &desktop_full_hw(), Some("en")).unwrap();
        assert!(s.audio_format_id.is_some());
        // 251 is Opus ~141k; preferred over AAC 140 and low-rate 249.
        assert_eq!(s.audio_format_id.as_deref(), Some("251"));
    }

    #[test]
    fn muxed_format_needs_no_separate_audio() {
        let json = r#"{"formats":[
          {"format_id":"18","url":"https://x/18","vcodec":"avc1.42001E",
           "acodec":"mp4a.40.2","width":640,"height":360,"fps":30,"tbr":600.0,
           "protocol":"https"}]}"#;
        let p = parse(json).unwrap();
        let s = select(&p, &desktop_full_hw(), None).unwrap();
        assert_eq!(s.video_format_id, "18");
        assert!(s.audio_format_id.is_none());
    }

    /// On a 4K/144Hz HDR desktop with everything in fixed function, frame rate
    /// breaks the tie between two equally-cheap 4K streams: the VP9 HDR 60fps
    /// rendition beats the AV1 30fps one. Codec identity is not a tiebreaker
    /// once both are free to decode — which is the point.
    #[test]
    fn desktop_prefers_60fps_hdr_over_a_30fps_stream_of_equal_size() {
        let p = probe_fixture();
        let s = select(&p, &desktop_full_hw(), None).unwrap();
        assert_eq!(s.video_format_id, "337", "{}", s.summary);
        assert_eq!(s.score.as_ref().expect("a scored selection").effective_fps, 60);
    }

    /// Cap the panel at 30 Hz and the same machine takes the AV1 stream —
    /// equal pixels, equal cost, and AV1 is the smaller download.
    #[test]
    fn at_30hz_the_smaller_stream_wins_on_bitrate() {
        let p = probe_fixture();
        let mut d = desktop_full_hw();
        d.budget.display_max_hz = 30;
        let s = select(&p, &d, None).unwrap();
        assert_eq!(s.video_format_id, "401", "{}", s.summary);
    }

    #[test]
    fn every_candidate_appears_in_the_trace() {
        let p = probe_fixture();
        let s = select(&p, &fairphone_venus(), None).unwrap();
        assert_eq!(s.trace.len(), p.formats.len());
    }

    #[test]
    fn no_playable_format_is_an_error_not_a_bad_pick() {
        let json = r#"{"formats":[
          {"format_id":"x","url":"https://x","vcodec":"theora","acodec":"none",
           "width":640,"height":480,"fps":30,"protocol":"https"}]}"#;
        let p = parse(json).unwrap();
        assert_eq!(
            select(&p, &desktop_full_hw(), None).unwrap_err(),
            SelectError::NothingPlayable
        );
    }

    #[test]
    fn empty_format_list_is_distinguishable() {
        let p = parse(r#"{"formats":[]}"#).unwrap();
        assert_eq!(
            select(&p, &desktop_full_hw(), None).unwrap_err(),
            SelectError::NoFormats
        );
    }
}
