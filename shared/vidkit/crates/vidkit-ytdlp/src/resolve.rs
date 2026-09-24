//! Turning a page URL into playable stream URLs, and keeping them alive.
//!
//! Format *selection* is in `select.rs`. This is the lifecycle around it:
//! invoking `yt-dlp`, caching the result, noticing when the signed URLs are
//! about to expire, and re-resolving without disturbing playback.
//!
//! The process invocation sits behind [`CommandRunner`] so all of that policy
//! is testable with canned JSON and no `yt-dlp` on the machine.
//!
//! # Why not construct URLs ourselves
//!
//! YouTube throttles any request whose `n` query parameter has not been
//! transformed by the player's JS. yt-dlp solves that (`nsig`); we cannot.
//! Hand-building or editing a media URL drops throughput to roughly 50 KB/s,
//! which presents as mysterious buffering rather than an error. So: take the
//! URL exactly as yt-dlp emits it, forward `http_headers` verbatim, and change
//! nothing.

use crate::probe::{parse, Probe, RawFormat};
use crate::select::{select, Selection};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use vidkit_core::caps::DeviceCaps;

/// `M:SS` or `H:MM:SS`, for listing durations next to titles.
pub fn fmt_secs(secs: f64) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Runs `yt-dlp`. Behind a trait so the policy above it is testable.
pub trait CommandRunner {
    /// Returns stdout on success. Implementations must not interpret the
    /// output — the caller parses it.
    fn run(&mut self, program: &str, args: &[String]) -> Result<Vec<u8>, RunError>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunError {
    NotFound,
    /// Non-zero exit. `stderr` is kept because yt-dlp's failure messages are
    /// the only useful diagnostic (age gate, geo block, sign-in required) and
    /// swallowing them makes support impossible.
    Failed { code: i32, stderr: String },
    Io(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    Run(RunError),
    Json(String),
    NoPlayableFormat,
    /// Live streams are segmented (HLS/DASH manifests) and need a different
    /// `MediaSource`. Fail clearly rather than half-working.
    LiveNotSupported,
}

/// Options that change the yt-dlp invocation.
#[derive(Clone, Debug, Default)]
pub struct ResolveOptions {
    /// `--cookies-from-browser <b>`. Needed for age-gated and members-only
    /// content; harmless otherwise, but it touches the user's browser profile,
    /// so it is opt-in.
    pub cookies_from_browser: Option<String>,
    /// `--cookies <path>`.
    pub cookies_file: Option<String>,
    /// Preferred audio language, passed to selection rather than to yt-dlp.
    pub prefer_language: Option<String>,
    /// Extra arguments, appended last. Escape hatch for extractor-args and
    /// proxy settings without growing this struct forever.
    pub extra_args: Vec<String>,
}

/// A resolved, playable pair of URLs.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub video_url: String,
    pub video_format_id: String,
    pub audio_url: Option<String>,
    pub audio_format_id: Option<String>,
    /// Forward these verbatim on every request. The signed URL is bound to
    /// them; a different User-Agent gets a 403.
    pub headers: BTreeMap<String, String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Earliest expiry across the chosen streams, if the URLs carry one.
    pub expires_at: Option<SystemTime>,
    pub summary: String,
    /// From the same `-J` call: the length, and captions that make whisper
    /// unnecessary. Both free -- the JSON was already fetched.
    pub duration: Option<std::time::Duration>,
    pub captions: Option<crate::probe::Caption>,
    /// The page's title, for history and search results.
    pub title: Option<String>,
}

/// Build the argument list.
///
/// Deliberately does **not** pass `-f`/`--format`: selection happens in this
/// crate against the device's real capabilities, and letting yt-dlp pick first
/// would hide the formats we need to choose between.
pub fn build_args(page_url: &str, opts: &ResolveOptions) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-J".into(),
        "--no-playlist".into(),
        "--no-warnings".into(),
        // Without this a single unavailable format aborts the whole probe.
        "--ignore-no-formats-error".into(),
    ];
    if let Some(b) = &opts.cookies_from_browser {
        a.push("--cookies-from-browser".into());
        a.push(b.clone());
    }
    if let Some(f) = &opts.cookies_file {
        a.push("--cookies".into());
        a.push(f.clone());
    }
    a.extend(opts.extra_args.iter().cloned());
    // URL last, and after a `--` guard so a URL beginning with `-` cannot be
    // read as a flag.
    a.push("--".into());
    a.push(page_url.into());
    a
}

/// Pull the `expire` parameter out of a signed media URL.
///
/// Google's `*.googlevideo.com` URLs carry `expire=<unix seconds>`. Other
/// extractors use different schemes or none, in which case this returns `None`
/// and the caller falls back to reacting to 403s instead of predicting them.
pub fn url_expiry(url: &str) -> Option<SystemTime> {
    let query = url.split_once('?').map(|(_, q)| q)?;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "expire" {
            let secs: u64 = v.parse().ok()?;
            return Some(UNIX_EPOCH + Duration::from_secs(secs));
        }
    }
    None
}

fn find<'a>(p: &'a Probe, id: &str) -> Option<&'a RawFormat> {
    p.formats.iter().find(|f| f.format_id == id)
}

fn duration_of(p: &Probe) -> Option<std::time::Duration> {
    p.duration.filter(|d| d.is_finite() && *d > 0.0).map(std::time::Duration::from_secs_f64)
}

fn build_resolved(probe: &Probe, sel: &Selection, lang: Option<&str>) -> Option<Resolved> {
    let v = find(probe, &sel.video_format_id)?;
    let a = sel.audio_format_id.as_deref().and_then(|id| find(probe, id));

    let v_exp = v.url.as_deref().and_then(url_expiry);
    let a_exp = a.and_then(|f| f.url.as_deref()).and_then(url_expiry);
    // Earliest wins: refreshing on the video URL while the audio one has
    // already expired gets you a silent video.
    let expires_at = match (v_exp, a_exp) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    };

    Some(Resolved {
        video_url: v.url.clone()?,
        video_format_id: v.format_id.clone(),
        audio_url: a.and_then(|f| f.url.clone()),
        audio_format_id: a.map(|f| f.format_id.clone()),
        headers: v.http_headers.clone(),
        width: v.width.unwrap_or(0),
        height: v.height.unwrap_or(0),
        fps: v.fps_or_guess(),
        expires_at,
        summary: sel.summary.clone(),
        duration: duration_of(probe),
        captions: crate::probe::pick_captions(probe, lang),
        title: probe.title.clone(),
    })
}

/// Owns the resolve/refresh lifecycle for one media item.
pub struct Resolver<R: CommandRunner> {
    runner: R,
    program: String,
    page_url: String,
    opts: ResolveOptions,
    current: Option<Resolved>,
    /// How long before expiry to refresh. Must exceed the worst-case yt-dlp
    /// runtime or the refresh lands after the URL is already dead.
    pub refresh_margin: Duration,
    pub resolve_count: u32,
}

impl<R: CommandRunner> Resolver<R> {
    pub fn new(runner: R, page_url: impl Into<String>, opts: ResolveOptions) -> Self {
        Self {
            runner,
            program: "yt-dlp".into(),
            page_url: page_url.into(),
            opts,
            current: None,
            refresh_margin: Duration::from_secs(120),
            resolve_count: 0,
        }
    }

    pub fn with_program(mut self, p: impl Into<String>) -> Self {
        self.program = p.into();
        self
    }

    pub fn current(&self) -> Option<&Resolved> {
        self.current.as_ref()
    }

    fn probe(&mut self) -> Result<Probe, ResolveError> {
        let args = build_args(&self.page_url, &self.opts);
        let out = self
            .runner
            .run(&self.program.clone(), &args)
            .map_err(ResolveError::Run)?;
        self.resolve_count += 1;
        let text = String::from_utf8_lossy(&out);
        parse(&text).map_err(|e| ResolveError::Json(e.to_string()))
    }

    /// First resolve. Picks a format against `caps`.
    pub fn resolve(&mut self, caps: &DeviceCaps) -> Result<&Resolved, ResolveError> {
        let probe = self.probe()?;
        if probe.is_live == Some(true) {
            return Err(ResolveError::LiveNotSupported);
        }
        let lang = self.opts.prefer_language.clone();
        let sel = select(&probe, caps, lang.as_deref()).map_err(|_| ResolveError::NoPlayableFormat)?;
        let r = build_resolved(&probe, &sel, lang.as_deref()).ok_or(ResolveError::NoPlayableFormat)?;
        self.current = Some(r);
        Ok(self.current.as_ref().expect("just set"))
    }

    /// Re-resolve an expired URL.
    ///
    /// Keeps the *same* format if it still exists, rather than re-running
    /// selection. A different format would mean a different codec or
    /// resolution mid-playback, forcing a decoder reconfiguration and a visible
    /// stall — for what is meant to be an invisible URL refresh. Only if the
    /// format has genuinely disappeared do we fall back to re-selecting.
    pub fn refresh(&mut self, caps: &DeviceCaps) -> Result<&Resolved, ResolveError> {
        let wanted = self.current.as_ref().map(|r| r.video_format_id.clone());
        let probe = self.probe()?;

        if let Some(id) = wanted {
            if let Some(v) = find(&probe, &id) {
                if v.url.is_some() {
                    let keep_audio = self.current.as_ref().and_then(|r| r.audio_format_id.clone());
                    let a = keep_audio.as_deref().and_then(|aid| find(&probe, aid));
                    let prev = self.current.as_ref().expect("wanted implies current");
                    let v_exp = v.url.as_deref().and_then(url_expiry);
                    let a_exp = a.and_then(|f| f.url.as_deref()).and_then(url_expiry);
                    let r = Resolved {
                        video_url: v.url.clone().expect("checked"),
                        video_format_id: v.format_id.clone(),
                        audio_url: a.and_then(|f| f.url.clone()),
                        audio_format_id: a.map(|f| f.format_id.clone()),
                        headers: v.http_headers.clone(),
                        width: v.width.unwrap_or(0),
                        height: v.height.unwrap_or(0),
                        fps: v.fps_or_guess(),
                        expires_at: match (v_exp, a_exp) {
                            (Some(x), Some(y)) => Some(x.min(y)),
                            (x, y) => x.or(y),
                        },
                        summary: prev.summary.clone(),
                        duration: duration_of(&probe),
                        captions: crate::probe::pick_captions(&probe, self.opts.prefer_language.as_deref()),
                        title: probe.title.clone(),
                    };
                    self.current = Some(r);
                    return Ok(self.current.as_ref().expect("just set"));
                }
            }
        }

        // Format is gone. Re-select and accept that the caller will have to
        // handle a format change.
        let lang = self.opts.prefer_language.clone();
        let sel = select(&probe, caps, lang.as_deref()).map_err(|_| ResolveError::NoPlayableFormat)?;
        let r = build_resolved(&probe, &sel, lang.as_deref()).ok_or(ResolveError::NoPlayableFormat)?;
        self.current = Some(r);
        Ok(self.current.as_ref().expect("just set"))
    }

    /// Should we refresh before the next read?
    ///
    /// `now` is passed in rather than read from the system clock so this is
    /// testable and so a paused player does not refresh on wall time it never
    /// used.
    pub fn needs_refresh(&self, now: SystemTime) -> bool {
        match self.current.as_ref().and_then(|r| r.expires_at) {
            // No expiry information: react to 403s instead of predicting.
            None => false,
            Some(exp) => now + self.refresh_margin >= exp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use vidkit_core::caps::{desktop_full_hw, fairphone_venus};

    struct ScriptedRunner {
        responses: VecDeque<Result<Vec<u8>, RunError>>,
        pub calls: Vec<Vec<String>>,
    }

    impl ScriptedRunner {
        fn new(responses: Vec<Result<Vec<u8>, RunError>>) -> Self {
            Self {
                responses: responses.into(),
                calls: Vec::new(),
            }
        }
    }

    impl CommandRunner for ScriptedRunner {
        fn run(&mut self, _program: &str, args: &[String]) -> Result<Vec<u8>, RunError> {
            self.calls.push(args.to_vec());
            self.responses
                .pop_front()
                .unwrap_or(Err(RunError::Io("script exhausted".into())))
        }
    }

    fn expiring_url(name: &str, expire: u64) -> String {
        format!("https://rr1---sn-abc.googlevideo.com/videoplayback?expire={expire}&ei=xyz&id={name}&n=SIGNED")
    }

    fn json(expire: u64) -> String {
        format!(
            r#"{{
              "id":"vid","title":"t","duration":100.0,"is_live":false,
              "formats":[
                {{"format_id":"251","url":"{}","vcodec":"none","acodec":"opus",
                  "abr":141.0,"protocol":"https","language":"en"}},
                {{"format_id":"248","url":"{}","vcodec":"vp09.00.40.08","acodec":"none",
                  "width":1920,"height":1080,"fps":30,"vbr":2800.0,"protocol":"https",
                  "dynamic_range":"SDR",
                  "http_headers":{{"User-Agent":"Mozilla/5.0 test","Accept-Language":"en"}}}}
              ]}}"#,
            expiring_url("audio", expire),
            expiring_url("video", expire)
        )
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    // ---- argument construction -----------------------------------------

    #[test]
    fn args_request_json_and_never_pin_a_format() {
        let a = build_args("https://x/watch?v=1", &ResolveOptions::default());
        assert!(a.contains(&"-J".to_string()));
        assert!(a.contains(&"--no-playlist".to_string()));
        assert!(
            !a.iter().any(|s| s == "-f" || s == "--format"),
            "selection is ours; yt-dlp must return every format: {a:?}"
        );
    }

    #[test]
    fn url_is_last_and_guarded_against_flag_injection() {
        let a = build_args("-oops", &ResolveOptions::default());
        assert_eq!(a[a.len() - 2], "--");
        assert_eq!(a[a.len() - 1], "-oops");
    }

    #[test]
    fn cookie_options_are_passed_through() {
        let a = build_args(
            "https://x",
            &ResolveOptions {
                cookies_from_browser: Some("firefox".into()),
                extra_args: vec!["--proxy".into(), "socks5://h".into()],
                ..Default::default()
            },
        );
        let i = a.iter().position(|s| s == "--cookies-from-browser").unwrap();
        assert_eq!(a[i + 1], "firefox");
        assert!(a.windows(2).any(|w| w[0] == "--proxy" && w[1] == "socks5://h"));
    }

    // ---- expiry --------------------------------------------------------

    #[test]
    fn expiry_is_parsed_from_a_googlevideo_url() {
        let e = url_expiry(&expiring_url("v", 1_800_000_000)).unwrap();
        assert_eq!(e, at(1_800_000_000));
    }

    #[test]
    fn urls_without_an_expire_parameter_report_none() {
        assert!(url_expiry("https://example.com/a.mp4").is_none());
        assert!(url_expiry("https://example.com/a.mp4?foo=bar").is_none());
    }

    #[test]
    fn a_garbage_expire_value_is_not_a_panic() {
        assert!(url_expiry("https://x/y?expire=soon").is_none());
    }

    // ---- resolve -------------------------------------------------------

    #[test]
    fn resolve_picks_a_format_and_carries_headers_and_expiry() {
        let r = ScriptedRunner::new(vec![Ok(json(1_800_000_000).into_bytes())]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        let out = res.resolve(&desktop_full_hw()).unwrap().clone();

        assert_eq!(out.video_format_id, "248");
        assert_eq!(out.audio_format_id.as_deref(), Some("251"));
        assert_eq!(
            out.headers.get("User-Agent").map(String::as_str),
            Some("Mozilla/5.0 test"),
            "headers must survive to the fetcher or every request 403s"
        );
        assert_eq!(out.expires_at, Some(at(1_800_000_000)));
        assert_eq!((out.width, out.height, out.fps), (1920, 1080, 30));
    }

    #[test]
    fn live_streams_are_rejected_clearly() {
        let live = r#"{"id":"x","is_live":true,"formats":[
          {"format_id":"1","url":"https://x/m3u8","vcodec":"avc1.640028","acodec":"none",
           "width":1920,"height":1080,"fps":30,"protocol":"m3u8_native"}]}"#;
        let r = ScriptedRunner::new(vec![Ok(live.into())]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        assert_eq!(
            res.resolve(&desktop_full_hw()).unwrap_err(),
            ResolveError::LiveNotSupported
        );
    }

    #[test]
    fn a_yt_dlp_failure_keeps_stderr_for_diagnosis() {
        let r = ScriptedRunner::new(vec![Err(RunError::Failed {
            code: 1,
            stderr: "Sign in to confirm your age".into(),
        })]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        match res.resolve(&desktop_full_hw()).unwrap_err() {
            ResolveError::Run(RunError::Failed { stderr, .. }) => {
                assert!(stderr.contains("age"), "stderr must not be swallowed");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        let r = ScriptedRunner::new(vec![Ok(b"not json".to_vec())]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        assert!(matches!(
            res.resolve(&desktop_full_hw()),
            Err(ResolveError::Json(_))
        ));
    }

    // ---- refresh -------------------------------------------------------

    #[test]
    fn refresh_margin_triggers_before_the_url_dies() {
        let r = ScriptedRunner::new(vec![Ok(json(1_000_000).into_bytes())]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        res.refresh_margin = Duration::from_secs(120);
        res.resolve(&desktop_full_hw()).unwrap();

        assert!(!res.needs_refresh(at(999_000)), "too early");
        assert!(res.needs_refresh(at(999_890)), "inside the 120s margin");
        assert!(res.needs_refresh(at(1_000_001)), "already expired");
    }

    #[test]
    fn no_expiry_information_means_no_predictive_refresh() {
        let no_exp = r#"{"formats":[
          {"format_id":"a","url":"https://cdn/x.mp4","vcodec":"avc1.640028","acodec":"mp4a.40.2",
           "width":1280,"height":720,"fps":30,"protocol":"https"}]}"#;
        let r = ScriptedRunner::new(vec![Ok(no_exp.into())]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        res.resolve(&desktop_full_hw()).unwrap();
        assert!(!res.needs_refresh(at(9_999_999_999)));
    }

    /// The point of refresh: a new URL for the *same* stream, so playback does
    /// not hiccup.
    #[test]
    fn refresh_keeps_the_same_format_and_swaps_only_the_url() {
        let r = ScriptedRunner::new(vec![
            Ok(json(1_000_000).into_bytes()),
            Ok(json(2_000_000).into_bytes()),
        ]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        let first = res.resolve(&desktop_full_hw()).unwrap().clone();

        let second = res.refresh(&desktop_full_hw()).unwrap().clone();
        assert_eq!(second.video_format_id, first.video_format_id);
        assert_eq!(second.audio_format_id, first.audio_format_id);
        assert_ne!(second.video_url, first.video_url, "URL must actually change");
        assert_eq!(second.expires_at, Some(at(2_000_000)));
        assert_eq!(res.resolve_count, 2);
    }

    /// Even if a *better* format has appeared, refresh must not switch to it
    /// mid-playback — that would force a decoder reconfiguration for what
    /// should be an invisible URL swap.
    #[test]
    fn refresh_does_not_upgrade_to_a_newly_available_format() {
        let with_4k = json(2_000_000).replace(
            r#""formats":["#,
            r#""formats":[
              {"format_id":"401","url":"https://cdn/4k","vcodec":"av01.0.12M.08","acodec":"none",
               "width":3840,"height":2160,"fps":30,"vbr":11000.0,"protocol":"https",
               "dynamic_range":"SDR"},"#,
        );
        let r = ScriptedRunner::new(vec![
            Ok(json(1_000_000).into_bytes()),
            Ok(with_4k.into_bytes()),
        ]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        res.resolve(&desktop_full_hw()).unwrap();
        let after = res.refresh(&desktop_full_hw()).unwrap().clone();
        assert_eq!(after.video_format_id, "248", "must not switch formats");
    }

    /// ...but if the format is genuinely gone, re-select rather than fail.
    #[test]
    fn refresh_reselects_when_the_format_disappeared() {
        let without_vp9 = r#"{"formats":[
          {"format_id":"251","url":"https://x/a?expire=2000000","vcodec":"none","acodec":"opus",
           "abr":141.0,"protocol":"https"},
          {"format_id":"137","url":"https://x/v?expire=2000000","vcodec":"avc1.640028",
           "acodec":"none","width":1920,"height":1080,"fps":30,"vbr":4200.0,"protocol":"https"}]}"#;
        let r = ScriptedRunner::new(vec![
            Ok(json(1_000_000).into_bytes()),
            Ok(without_vp9.into()),
        ]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        res.resolve(&desktop_full_hw()).unwrap();
        let after = res.refresh(&desktop_full_hw()).unwrap().clone();
        assert_eq!(after.video_format_id, "137");
    }

    #[test]
    fn earliest_expiry_across_video_and_audio_wins() {
        // Audio expires first; refreshing on the video URL alone would give a
        // silent video.
        let mixed = format!(
            r#"{{"formats":[
              {{"format_id":"251","url":"{}","vcodec":"none","acodec":"opus","abr":141.0,
                "protocol":"https"}},
              {{"format_id":"248","url":"{}","vcodec":"vp09.00.40.08","acodec":"none",
                "width":1920,"height":1080,"fps":30,"vbr":2800.0,"protocol":"https"}}]}}"#,
            expiring_url("audio", 1_000_000),
            expiring_url("video", 9_000_000)
        );
        let r = ScriptedRunner::new(vec![Ok(mixed.into_bytes())]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        let out = res.resolve(&desktop_full_hw()).unwrap().clone();
        assert_eq!(out.expires_at, Some(at(1_000_000)));
    }

    /// Selection still runs against the device, so the phone and the desktop
    /// resolve the same page differently.
    #[test]
    fn resolution_is_device_dependent() {
        let r = ScriptedRunner::new(vec![Ok(json(1_000_000).into_bytes())]);
        let mut res = Resolver::new(r, "https://x", ResolveOptions::default());
        let phone = res.resolve(&fairphone_venus()).unwrap().clone();
        assert!(
            phone.summary.contains("Fixed") || phone.summary.contains("V4l2Stateful"),
            "phone should land on a hardware path: {}",
            phone.summary
        );
    }
}
