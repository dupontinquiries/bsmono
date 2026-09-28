//! snap-handoff — how a photo gets from another app to a snapclone contact.
//!
//! snapshot-cam takes photos and keeps them local; snapclone knows how to
//! reach a person. Neither links the other. What joins them is a directory
//! and two file shapes, and this crate is the only place either side
//! spells them:
//!
//! ```text
//! $XDG_DATA_HOME/snapclone/outbox/
//!   <id>.request    who to send what: an INI written by the camera app
//!   <id>.sent       written by snapclone when the transport confirmed
//!   <id>.failed     written by snapclone with the reason
//! ```
//!
//! The photo itself stays where the camera app put it; the request names
//! it by absolute path. snapclone reads the bytes, sends them through its
//! ordinary snap path, and answers with a marker. The camera app polls for
//! the marker, records the outcome on its roll, and removes the request
//! and the marker. Nothing here is a queue with ordering promises: each
//! request is independent, and a request nobody answers stays visibly
//! pending — which is the truth, and the state a UI should show.
//!
//! ## Why files
//!
//! * Both apps already run as separate processes on the same device; a
//!   directory is the IPC that needs no daemon, no bus name, no schema
//!   negotiation, and survives either process restarting.
//! * A person can `ls` it. A stuck handoff is a file with no marker, not
//!   a message lost in a socket.
//! * Atomic: requests are written to a temp name and renamed, so snapclone
//!   never reads half a request.
//!
//! ## What it does not promise
//!
//! Delivery. A `.sent` marker means snapclone's transport reported
//! success; whether the recipient opened it is snapclone's business.
//! Disclosure begins at the request, and the camera app's roll treats it
//! that way (pending already counts as "somebody may have it").

use std::path::{Path, PathBuf};

/// A request to send one file to one contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Unique per request; the file stem of the three files.
    pub id: String,
    /// The contact's snapclone sc-ID.
    pub to: String,
    /// Absolute path of the photo to send.
    pub file: PathBuf,
    /// Unix milliseconds the photo was taken, for the snap's own timestamp.
    pub taken_ms: u64,
    /// The app that asked, for the log.
    pub from: String,
}

/// What snapclone answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    Failed(String),
}

/// The outbox snapclone watches.
pub fn default_dir() -> PathBuf {
    config_kit::data_dir("snapclone").join("outbox")
}

/// A fresh request id: time-ordered, unique enough for one device.
pub fn new_id(now_ms: u64) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!("{now_ms}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

impl Request {
    pub fn render(&self) -> String {
        format!("# snap-handoff request\nto = {}\nfile = {}\ntaken_ms = {}\nfrom = {}\n",
                self.to, self.file.display(), self.taken_ms, self.from)
    }

    /// Parse a request file's text. `None` when a required line is missing
    /// — a request without a recipient or a file is not sendable, and
    /// guessing either would send a photo to the wrong place.
    pub fn parse(id: &str, text: &str) -> Option<Request> {
        let ini = config_kit::Ini::parse(text);
        let get = |k: &str| ini.get("", k).or_else(|| ini.get("general", k)).map(str::trim).filter(|v| !v.is_empty());
        let to = get("to")?.to_string();
        let file = PathBuf::from(get("file")?);
        if !file.is_absolute() { return None; }
        Some(Request {
            id: id.to_string(), to, file,
            taken_ms: get("taken_ms").and_then(|v| v.parse().ok()).unwrap_or(0),
            from: get("from").unwrap_or("unknown").to_string(),
        })
    }
}

fn request_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.request")) }
fn sent_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.sent")) }
fn failed_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.failed")) }

/// Drop a request in the outbox. Atomic: temp file, then rename.
pub fn write_request(dir: &Path, req: &Request) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let final_path = request_path(dir, &req.id);
    let tmp = dir.join(format!("{}.request.tmp", req.id));
    std::fs::write(&tmp, req.render())?;
    std::fs::rename(&tmp, &final_path)?;
    Ok(final_path)
}

/// Every request without an outcome yet, oldest id first.
pub fn pending(dir: &Path) -> Vec<Request> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Request> = rd.flatten().filter_map(|e| {
        let p = e.path();
        let name = p.file_name()?.to_str()?;
        let id = name.strip_suffix(".request")?;
        if outcome(dir, id).is_some() { return None; }
        let text = std::fs::read_to_string(&p).ok()?;
        Request::parse(id, &text)
    }).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// snapclone's answer for a request, if it has answered.
pub fn outcome(dir: &Path, id: &str) -> Option<Outcome> {
    if sent_path(dir, id).exists() { return Some(Outcome::Sent); }
    let f = failed_path(dir, id);
    if f.exists() {
        let why = std::fs::read_to_string(&f).unwrap_or_default();
        let why = why.trim();
        return Some(Outcome::Failed(if why.is_empty() { "failed".into() } else { why.to_string() }));
    }
    None
}

/// snapclone writes the answer.
pub fn mark(dir: &Path, id: &str, outcome: &Outcome) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    match outcome {
        Outcome::Sent => std::fs::write(sent_path(dir, id), "sent\n"),
        Outcome::Failed(why) => std::fs::write(failed_path(dir, id), format!("{why}\n")),
    }
}

/// The camera app has recorded the outcome: remove the request and its
/// marker so the outbox holds only live work.
pub fn finish(dir: &Path, id: &str) {
    for p in [request_path(dir, id), sent_path(dir, id), failed_path(dir, id)] {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str) -> Request {
        Request { id: id.into(), to: "sc-2dd464a4".into(), file: PathBuf::from("/home/u/Pictures/cam-1.jpg"),
                  taken_ms: 1_700_000_000_000, from: "snapshot-cam".into() }
    }

    #[test]
    fn a_request_round_trips_through_its_file_text() {
        let r = req("1700000000000-1-0");
        assert_eq!(Request::parse(&r.id, &r.render()), Some(r.clone()));
        assert_eq!(Request::parse("x", "to = sc-a\n"), None, "no file: not sendable");
        assert_eq!(Request::parse("x", "file = /a.jpg\n"), None, "no recipient: not sendable");
        assert_eq!(Request::parse("x", "to = sc-a\nfile = relative.jpg\n"), None, "a relative path is ambiguous between two apps");
    }

    #[test]
    fn the_outbox_walk_lists_unanswered_requests_oldest_first_and_drops_answered_ones() {
        let d = tempfile::tempdir().unwrap();
        write_request(d.path(), &req("2-b")).unwrap();
        write_request(d.path(), &req("1-a")).unwrap();
        write_request(d.path(), &req("3-c")).unwrap();
        std::fs::write(d.path().join("junk.txt"), "x").unwrap();
        let ids: Vec<String> = pending(d.path()).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["1-a", "2-b", "3-c"]);
        mark(d.path(), "2-b", &Outcome::Sent).unwrap();
        mark(d.path(), "3-c", &Outcome::Failed("peer unreachable".into())).unwrap();
        let ids: Vec<String> = pending(d.path()).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["1-a"]);
        assert_eq!(outcome(d.path(), "2-b"), Some(Outcome::Sent));
        assert_eq!(outcome(d.path(), "3-c"), Some(Outcome::Failed("peer unreachable".into())));
        assert_eq!(outcome(d.path(), "1-a"), None);
    }

    #[test]
    fn finishing_removes_the_request_and_its_marker() {
        let d = tempfile::tempdir().unwrap();
        write_request(d.path(), &req("9")).unwrap();
        mark(d.path(), "9", &Outcome::Sent).unwrap();
        finish(d.path(), "9");
        assert!(pending(d.path()).is_empty());
        assert_eq!(outcome(d.path(), "9"), None);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0, "nothing left behind");
    }

    #[test]
    fn a_missing_outbox_is_empty_not_an_error() {
        assert!(pending(Path::new("/nonexistent/outbox-xyz")).is_empty());
    }

    #[test]
    fn ids_are_time_ordered_and_distinct() {
        let a = new_id(1000);
        let b = new_id(1000);
        let c = new_id(2000);
        assert_ne!(a, b);
        assert!(a < c && b < c);
    }
}
//! snap-handoff — how a photo gets from another app to a snapclone contact.
//!
//! snapshot-cam takes photos and keeps them local; snapclone knows how to
//! reach a person. Neither links the other. What joins them is a directory
//! and two file shapes, and this crate is the only place either side
//! spells them:
//!
//! ```text
//! $XDG_DATA_HOME/snapclone/outbox/
//!   <id>.request    who to send what: an INI written by the camera app
//!   <id>.sent       written by snapclone when the transport confirmed
//!   <id>.failed     written by snapclone with the reason
//! ```
//!
//! The photo itself stays where the camera app put it; the request names
//! it by absolute path. snapclone reads the bytes, sends them through its
//! ordinary snap path, and answers with a marker. The camera app polls for
//! the marker, records the outcome on its roll, and removes the request
//! and the marker. Nothing here is a queue with ordering promises: each
//! request is independent, and a request nobody answers stays visibly
//! pending — which is the truth, and the state a UI should show.
//!
//! ## Why files
//!
//! * Both apps already run as separate processes on the same device; a
//!   directory is the IPC that needs no daemon, no bus name, no schema
//!   negotiation, and survives either process restarting.
//! * A person can `ls` it. A stuck handoff is a file with no marker, not
//!   a message lost in a socket.
//! * Atomic: requests are written to a temp name and renamed, so snapclone
//!   never reads half a request.
//!
//! ## What it does not promise
//!
//! Delivery. A `.sent` marker means snapclone's transport reported
//! success; whether the recipient opened it is snapclone's business.
//! Disclosure begins at the request, and the camera app's roll treats it
//! that way (pending already counts as "somebody may have it").

use std::path::{Path, PathBuf};

/// A request to send one file to one contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// Unique per request; the file stem of the three files.
    pub id: String,
    /// The contact's snapclone sc-ID.
    pub to: String,
    /// Absolute path of the photo to send.
    pub file: PathBuf,
    /// Unix milliseconds the photo was taken, for the snap's own timestamp.
    pub taken_ms: u64,
    /// The app that asked, for the log.
    pub from: String,
}

/// What snapclone answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    Failed(String),
}

/// The outbox snapclone watches.
pub fn default_dir() -> PathBuf {
    config_kit::data_dir("snapclone").join("outbox")
}

/// A fresh request id: time-ordered, unique enough for one device.
pub fn new_id(now_ms: u64) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!("{now_ms}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

impl Request {
    pub fn render(&self) -> String {
        format!("# snap-handoff request\nto = {}\nfile = {}\ntaken_ms = {}\nfrom = {}\n",
                self.to, self.file.display(), self.taken_ms, self.from)
    }

    /// Parse a request file's text. `None` when a required line is missing
    /// — a request without a recipient or a file is not sendable, and
    /// guessing either would send a photo to the wrong place.
    pub fn parse(id: &str, text: &str) -> Option<Request> {
        let ini = config_kit::Ini::parse(text);
        let get = |k: &str| ini.get("", k).or_else(|| ini.get("general", k)).map(str::trim).filter(|v| !v.is_empty());
        let to = get("to")?.to_string();
        let file = PathBuf::from(get("file")?);
        if !file.is_absolute() { return None; }
        Some(Request {
            id: id.to_string(), to, file,
            taken_ms: get("taken_ms").and_then(|v| v.parse().ok()).unwrap_or(0),
            from: get("from").unwrap_or("unknown").to_string(),
        })
    }
}

fn request_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.request")) }
fn sent_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.sent")) }
fn failed_path(dir: &Path, id: &str) -> PathBuf { dir.join(format!("{id}.failed")) }

/// Drop a request in the outbox. Atomic: temp file, then rename.
pub fn write_request(dir: &Path, req: &Request) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let final_path = request_path(dir, &req.id);
    let tmp = dir.join(format!("{}.request.tmp", req.id));
    std::fs::write(&tmp, req.render())?;
    std::fs::rename(&tmp, &final_path)?;
    Ok(final_path)
}

/// Every request without an outcome yet, oldest id first.
pub fn pending(dir: &Path) -> Vec<Request> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Request> = rd.flatten().filter_map(|e| {
        let p = e.path();
        let name = p.file_name()?.to_str()?;
        let id = name.strip_suffix(".request")?;
        if outcome(dir, id).is_some() { return None; }
        let text = std::fs::read_to_string(&p).ok()?;
        Request::parse(id, &text)
    }).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// snapclone's answer for a request, if it has answered.
pub fn outcome(dir: &Path, id: &str) -> Option<Outcome> {
    if sent_path(dir, id).exists() { return Some(Outcome::Sent); }
    let f = failed_path(dir, id);
    if f.exists() {
        let why = std::fs::read_to_string(&f).unwrap_or_default();
        let why = why.trim();
        return Some(Outcome::Failed(if why.is_empty() { "failed".into() } else { why.to_string() }));
    }
    None
}

/// snapclone writes the answer.
pub fn mark(dir: &Path, id: &str, outcome: &Outcome) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    match outcome {
        Outcome::Sent => std::fs::write(sent_path(dir, id), "sent\n"),
        Outcome::Failed(why) => std::fs::write(failed_path(dir, id), format!("{why}\n")),
    }
}

/// The camera app has recorded the outcome: remove the request and its
/// marker so the outbox holds only live work.
pub fn finish(dir: &Path, id: &str) {
    for p in [request_path(dir, id), sent_path(dir, id), failed_path(dir, id)] {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str) -> Request {
        Request { id: id.into(), to: "sc-2dd464a4".into(), file: PathBuf::from("/home/u/Pictures/cam-1.jpg"),
                  taken_ms: 1_700_000_000_000, from: "snapshot-cam".into() }
    }

    #[test]
    fn a_request_round_trips_through_its_file_text() {
        let r = req("1700000000000-1-0");
        assert_eq!(Request::parse(&r.id, &r.render()), Some(r.clone()));
        assert_eq!(Request::parse("x", "to = sc-a\n"), None, "no file: not sendable");
        assert_eq!(Request::parse("x", "file = /a.jpg\n"), None, "no recipient: not sendable");
        assert_eq!(Request::parse("x", "to = sc-a\nfile = relative.jpg\n"), None, "a relative path is ambiguous between two apps");
    }

    #[test]
    fn the_outbox_walk_lists_unanswered_requests_oldest_first_and_drops_answered_ones() {
        let d = tempfile::tempdir().unwrap();
        write_request(d.path(), &req("2-b")).unwrap();
        write_request(d.path(), &req("1-a")).unwrap();
        write_request(d.path(), &req("3-c")).unwrap();
        std::fs::write(d.path().join("junk.txt"), "x").unwrap();
        let ids: Vec<String> = pending(d.path()).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["1-a", "2-b", "3-c"]);
        mark(d.path(), "2-b", &Outcome::Sent).unwrap();
        mark(d.path(), "3-c", &Outcome::Failed("peer unreachable".into())).unwrap();
        let ids: Vec<String> = pending(d.path()).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["1-a"]);
        assert_eq!(outcome(d.path(), "2-b"), Some(Outcome::Sent));
        assert_eq!(outcome(d.path(), "3-c"), Some(Outcome::Failed("peer unreachable".into())));
        assert_eq!(outcome(d.path(), "1-a"), None);
    }

    #[test]
    fn finishing_removes_the_request_and_its_marker() {
        let d = tempfile::tempdir().unwrap();
        write_request(d.path(), &req("9")).unwrap();
        mark(d.path(), "9", &Outcome::Sent).unwrap();
        finish(d.path(), "9");
        assert!(pending(d.path()).is_empty());
        assert_eq!(outcome(d.path(), "9"), None);
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0, "nothing left behind");
    }

    #[test]
    fn a_missing_outbox_is_empty_not_an_error() {
        assert!(pending(Path::new("/nonexistent/outbox-xyz")).is_empty());
    }

    #[test]
    fn ids_are_time_ordered_and_distinct() {
        let a = new_id(1000);
        let b = new_id(1000);
        let c = new_id(2000);
        assert_ne!(a, b);
        assert!(a < c && b < c);
    }
}
