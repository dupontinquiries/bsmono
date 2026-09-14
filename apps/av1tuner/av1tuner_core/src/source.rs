//! source — choosing where the footage is, without a UI.
//!
//! The CLI and the egui app both need to answer the same questions: is
//! this a folder or a server, can it be reached, does the server do
//! range requests, and what is in it. Doing that twice would give two
//! behaviours, and the GUI's would be the untested one.
//!
//! So the states and transitions live here and both shells drive them.
//! A panel becomes a match on [`Status`] and a couple of buttons.
//!
//! ## Why the range check is a state and not a detail
//!
//! Camera QuickTime puts its index (`moov`) at the very END of the
//! file -- measured at 100% of the way into one of these. A client must
//! read the end before it can decode the beginning. With HTTP range
//! support that is one small request; without it, reading the index
//! means pulling the whole file, and a 5.8 GB accidental download looks
//! exactly like "the app is being slow".
//!
//! That failure has to be a visible state with an explanation, not a
//! spinner.

use crate::remote::{Remote, Source};

/// Where the footage might be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A path on this machine.
    Folder,
    /// A dufs listing over HTTP.
    Dufs,
}

/// Work out which, from what the person typed.
pub fn kind_of(input: &str) -> Kind {
    let t = input.trim();
    if t.starts_with("http://") || t.starts_with("https://") { Kind::Dufs } else { Kind::Folder }
}

/// What a source picker is currently showing.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Nothing chosen.
    Empty,
    /// Working on it. Carries what it is doing, because "loading" with
    /// no verb is how a 5.8 GB download hides.
    Checking(String),
    /// Ready, with what was found.
    Ready(Ready),
    /// Not usable, with why and what to do.
    Problem { what: String, fix: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ready {
    pub kind: Kind,
    pub label: String,
    pub videos: usize,
    pub total_bytes: u64,
    /// Clips small enough to be false starts.
    pub stubs: Vec<String>,
    /// Days the shoot covers, from the camera's filenames.
    pub sessions: Vec<String>,
}

impl Ready {
    pub fn total_gb(&self) -> f64 { self.total_bytes as f64 / 1e9 }

    /// One line for a panel header.
    pub fn line(&self) -> String {
        let mut s = format!("{} clip(s), {:.1} GB", self.videos, self.total_gb());
        if self.sessions.len() > 1 { s.push_str(&format!(", {} days", self.sessions.len())); }
        if !self.stubs.is_empty() {
            s.push_str(&format!(", {} tiny", self.stubs.len()));
        }
        s
    }
}

impl Status {
    pub fn ready(&self) -> Option<&Ready> {
        match self { Status::Ready(r) => Some(r), _ => None }
    }
    pub fn busy(&self) -> bool { matches!(self, Status::Checking(_)) }
}

/// What the shell must go and do. Returned rather than performed, so
/// the state machine stays testable and the shell decides whether to
/// block, spawn a thread, or drive an async runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    /// Read a directory.
    ListFolder(String),
    /// Fetch a URL's body.
    FetchListing(String),
    /// HEAD one file with a Range header and report the status code.
    CheckRange(String),
}

/// The picker.
#[derive(Debug, Clone, PartialEq)]
pub struct Picker {
    pub input: String,
    pub status: Status,
    /// Files found, before the range check has passed.
    pending: Vec<Remote>,
}

impl Default for Picker {
    fn default() -> Self {
        Picker { input: String::new(), status: Status::Empty, pending: Vec::new() }
    }
}

/// Below this, a clip is a false start or a card test rather than a
/// take. Named rather than transcoded.
pub const STUB_BYTES: u64 = 100_000_000;

impl Picker {
    /// The person changed the text or pressed go.
    pub fn submit(&mut self) -> Action {
        let t = self.input.trim().to_string();
        if t.is_empty() {
            self.status = Status::Empty;
            return Action::Nothing;
        }
        match kind_of(&t) {
            Kind::Folder => {
                self.status = Status::Checking(format!("reading {t}"));
                Action::ListFolder(t)
            }
            Kind::Dufs => {
                self.status = Status::Checking(format!("asking {t} what it has"));
                Action::FetchListing(t)
            }
        }
    }

    /// A folder listing came back.
    pub fn folder_listed(&mut self, files: Vec<Remote>) -> Action {
        if files.iter().all(|f| !f.is_video()) {
            self.status = Status::Problem {
                what: "no video files here".into(),
                fix: Some("pick the folder the camera wrote to".into()),
            };
            return Action::Nothing;
        }
        self.finish(Kind::Folder, files);
        Action::Nothing
    }

    /// A dufs page came back.
    pub fn listing_fetched(&mut self, html: &str, base: &str) -> Action {
        let (files, notes) = crate::remote::parse_listing(html, base);
        let videos: Vec<Remote> = files.iter().filter(|f| f.is_video()).cloned().collect();
        if videos.is_empty() {
            self.status = Status::Problem {
                what: if notes.is_empty() { "no video files on that page".into() }
                      else { notes[0].clone() },
                fix: Some("check the URL points at the folder, not a file".into()),
            };
            return Action::Nothing;
        }
        // THE RANGE CHECK BEFORE ANYTHING ELSE, because without it the
        // first probe silently downloads a whole file.
        let probe = videos[0].url.clone();
        self.pending = files;
        self.status = Status::Checking("checking the server supports range requests".into());
        Action::CheckRange(probe)
    }

    /// The range probe answered. 206 means it honoured the range.
    pub fn range_checked(&mut self, http_status: u16) -> Action {
        if http_status != 206 {
            self.status = Status::Problem {
                what: format!("the server answered {http_status} to a range request, not 206"),
                fix: Some(crate::remote::Readiness::NoRangeRequests.explain()),
            };
            self.pending.clear();
            return Action::Nothing;
        }
        let files = std::mem::take(&mut self.pending);
        self.finish(Kind::Dufs, files);
        Action::Nothing
    }

    /// Something went wrong out in the shell.
    pub fn failed(&mut self, what: impl Into<String>) {
        self.status = Status::Problem { what: what.into(), fix: None };
        self.pending.clear();
    }

    fn finish(&mut self, kind: Kind, files: Vec<Remote>) {
        let src = Source { base: self.input.trim().to_string(), files };
        let videos = src.videos();
        let sessions: Vec<String> = crate::remote::by_session(&src).keys().cloned().collect();
        self.status = Status::Ready(Ready {
            kind,
            label: self.input.trim().to_string(),
            videos: videos.len(),
            total_bytes: src.total_bytes(),
            stubs: src.stubs(STUB_BYTES).iter().map(|f| f.name.clone()).collect(),
            sessions,
        });
    }
}

/// Filter a list of clip names, for a fuzzy picker.
///
/// Subsequence rather than substring: "c62" should find
/// `..._0929_C0062.mov`, which no substring match does.
pub fn matches(name: &str, query: &str) -> bool {
    if query.is_empty() { return true; }
    let (n, q) = (name.to_lowercase(), query.to_lowercase());
    let mut chars = n.chars();
    q.chars().all(|c| chars.any(|x| x == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, bytes: u64) -> Remote {
        Remote { url: format!("http://h/{name}"), name: name.into(), bytes, modified: None }
    }

    const PAGE: &str = r#"
<td class="path cell-name"><a href="http://h:5001/F/A_2026-09-10_0929_C0062.mov">A_2026-09-10_0929_C0062.mov</a></td>
<td class="cell-mtime">2026-09-10 10:29</td><td class="cell-size">5.8 GB</td>
<td class="path cell-name"><a href="http://h:5001/F/A_2026-09-10_0934_C0063.mov">A_2026-09-10_0934_C0063.mov</a></td>
<td class="cell-mtime">2026-09-10 10:34</td><td class="cell-size">61.2 MB</td>"#;

    #[test]
    fn a_url_and_a_path_take_different_routes() {
        assert_eq!(kind_of("http://192.168.2.221:5001/F/"), Kind::Dufs);
        assert_eq!(kind_of("/mnt/card/DCIM"), Kind::Folder);
        assert_eq!(kind_of("  ./footage "), Kind::Folder);
        let mut p = Picker { input: "http://h:5001/F/".into(), ..Default::default() };
        assert!(matches!(p.submit(), Action::FetchListing(_)));
        assert!(p.status.busy());
        let mut q = Picker { input: "/tmp/x".into(), ..Default::default() };
        assert!(matches!(q.submit(), Action::ListFolder(_)));
    }

    #[test]
    fn the_range_check_happens_before_the_source_is_usable() {
        // without it the first probe silently downloads a whole 5.8 GB
        // file to read the index at the end
        let mut p = Picker { input: "http://h:5001/F/".into(), ..Default::default() };
        p.submit();
        let a = p.listing_fetched(PAGE, "http://h:5001/F/");
        match a {
            Action::CheckRange(url) => assert!(url.contains("C0062"), "{url}"),
            o => panic!("expected a range check, got {o:?}"),
        }
        assert!(p.status.ready().is_none(), "not usable until the check passes");
        p.range_checked(206);
        let r = p.status.ready().expect("ready now");
        assert_eq!(r.videos, 2);
        assert_eq!(r.kind, Kind::Dufs);
    }

    #[test]
    fn a_server_without_ranges_says_what_will_go_wrong() {
        let mut p = Picker { input: "http://h:5001/F/".into(), ..Default::default() };
        p.submit();
        p.listing_fetched(PAGE, "http://h:5001/F/");
        p.range_checked(200);
        match &p.status {
            Status::Problem { what, fix } => {
                assert!(what.contains("200") && what.contains("not 206"), "{what}");
                let fix = fix.as_deref().unwrap_or("");
                assert!(fix.contains("moov") && fix.contains("END"), "{fix}");
                assert!(fix.contains("download the whole file"), "the consequence: {fix}");
                assert!(fix.contains("dufs"), "and what to use: {fix}");
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_busy_state_always_says_what_it_is_doing() {
        // "loading" with no verb is how a 5.8 GB download hides
        let mut p = Picker { input: "http://h:5001/F/".into(), ..Default::default() };
        p.submit();
        match &p.status {
            Status::Checking(msg) => assert!(msg.len() > 10 && msg.contains("h:5001"), "{msg}"),
            o => panic!("{o:?}"),
        }
        p.listing_fetched(PAGE, "http://h:5001/F/");
        match &p.status {
            Status::Checking(msg) => assert!(msg.contains("range requests"), "{msg}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn a_folder_with_no_video_in_it_says_so_rather_than_showing_empty() {
        let mut p = Picker { input: "/tmp/x".into(), ..Default::default() };
        p.submit();
        p.folder_listed(vec![f("notes.txt", 900), f("log.csv", 40)]);
        match &p.status {
            Status::Problem { what, fix } => {
                assert!(what.contains("no video files"), "{what}");
                assert!(fix.as_deref().unwrap_or("").contains("camera wrote to"));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn the_summary_names_the_tiny_clips_and_the_days() {
        let mut p = Picker { input: "http://h:5001/F/".into(), ..Default::default() };
        p.submit();
        p.listing_fetched(PAGE, "http://h:5001/F/");
        p.range_checked(206);
        let r = p.status.ready().unwrap();
        assert_eq!(r.stubs.len(), 1, "the 61 MB clip is a false start");
        assert!(r.stubs[0].contains("C0063"));
        assert_eq!(r.sessions, vec!["2026-09-10"]);
        let line = r.line();
        assert!(line.contains("2 clip(s)") && line.contains("tiny"), "{line}");
    }

    #[test]
    fn the_filter_finds_a_clip_by_its_number() {
        // "c62" should find A_2026-09-10_0929_C0062.mov, which no
        // substring match does
        let n = "A_2026-09-10_0929_C0062.mov";
        assert!(matches(n, "c62"));
        assert!(matches(n, "0929"));
        assert!(matches(n, ""), "an empty query matches everything");
        assert!(!matches(n, "c99"));
        assert!(matches(n, "C0062"), "and an exact one still works");
    }

    #[test]
    fn a_failure_out_in_the_shell_lands_somewhere_visible() {
        let mut p = Picker { input: "http://nope/".into(), ..Default::default() };
        p.submit();
        p.failed("could not resolve host");
        assert!(matches!(p.status, Status::Problem { .. }));
        assert!(!p.status.busy(), "and it stops spinning");
    }
}
