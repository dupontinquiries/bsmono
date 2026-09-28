//! share — who may see which photos.
//!
//! The same model `weather-share` gives ambient-learner, because it is the
//! right one for photos too: sharing is **per photo, per contact, and
//! revocable**. Each contact has a private namespace in a
//! [`datashare_core::Hub`] — `ph:{owner}:{contact}` — and a share is a
//! `Put` of the photo's record there, a revoke a `Delete`. A contact sees
//! exactly what was shared with them and nothing else; two contacts never
//! see each other's grants.
//!
//! ## What travels
//!
//! A [`PhotoRecord`]: name, size, hash, tags, when. The BYTES do not go
//! through the hub — a hub value is a string and a photo is megabytes.
//! Delivery of the file itself is snapclone's snap transport, which this
//! app does not link; what this app can do today is keep the grant table
//! that says what the transport is allowed to carry, and it says so on
//! the page rather than showing a "sent" that never happened.
//!
//! ## Contacts
//!
//! snapclone's contact book, read from its own file (`contacts.conf`, one
//! INI section per sc-ID) with `config-kit`. Read only: this app never
//! writes a contact, and never depends on the snapclone crate to read one.

use datashare_core::{Hub, Op, SubId};
use std::collections::{HashMap, HashSet};

/// What a contact learns about a shared photo.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoRecord {
    pub name: String,
    pub size: u64,
    pub dhash: u64,
    /// Written tags only. The derived ones describe the pixels the
    /// recipient will have anyway.
    pub tags: Vec<String>,
    /// Unix seconds the photo was last modified.
    pub at: i64,
}

impl PhotoRecord {
    /// Unit-separated, like `WeatherRecord`, so it round-trips through
    /// the hub's string values. Tags are comma-joined; a comma in a tag
    /// was never allowed by `Library::add_tag`'s callers anyway.
    pub fn encode(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{:016x}\u{1f}{}\u{1f}{}",
                self.name, self.size, self.dhash, self.tags.join(","), self.at)
    }

    pub fn decode(s: &str) -> Option<PhotoRecord> {
        let mut p = s.split('\u{1f}');
        let name = p.next()?.to_string();
        let size = p.next()?.parse().ok()?;
        let dhash = u64::from_str_radix(p.next()?, 16).ok()?;
        let tags: Vec<String> = p.next()?.split(',').filter(|t| !t.is_empty()).map(String::from).collect();
        let at = p.next()?.parse().ok()?;
        Some(PhotoRecord { name, size, dhash, tags, at })
    }
}

/// A snapclone contact, as much of it as sharing needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub sc_id: String,
    pub nickname: String,
}

impl Contact {
    /// The name to show: the nickname, or the id when there is none.
    pub fn label(&self) -> &str {
        if self.nickname.trim().is_empty() { &self.sc_id } else { &self.nickname }
    }
}

/// Parse snapclone's `contacts.conf`.
pub fn parse_contacts(text: &str) -> Vec<Contact> {
    let ini = config_kit::Ini::parse(text);
    let mut out: Vec<(u64, Contact)> = ini.sections.keys()
        .filter(|s| s.starts_with("sc-"))
        .map(|s| {
            let added: u64 = ini.get(s, "added_ms").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
            (added, Contact { sc_id: s.clone(), nickname: ini.get(s, "nickname").unwrap_or("").trim().to_string() })
        })
        .collect();
    // Oldest first, as snapclone lists them.
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sc_id.cmp(&b.1.sc_id)));
    out.into_iter().map(|(_, c)| c).collect()
}

/// snapclone's contacts on this machine, if it has any.
pub fn load_contacts() -> Vec<Contact> {
    config_kit::load("snapclone", "contacts.conf").map(|t| parse_contacts(&t)).unwrap_or_default()
}

/// The owner's exposed photo DB and the grants over it.
pub struct PhotoShare {
    hub: Hub,
    owner: String,
    owner_sub: SubId,
    /// photo key (its path) -> contacts it is shared with.
    grants: HashMap<String, HashSet<String>>,
}

impl PhotoShare {
    pub fn new(owner: impl Into<String>) -> Self {
        PhotoShare { hub: Hub::new(), owner: owner.into(), owner_sub: 1, grants: HashMap::new() }
    }

    fn ns_for(&self, contact: &str) -> String { format!("ph:{}:{}", self.owner, contact) }

    /// Share a photo with a contact. Idempotent.
    pub fn share_with(&mut self, contact: &str, key: &str, rec: &PhotoRecord) -> Vec<(SubId, Op)> {
        self.grants.entry(key.to_string()).or_default().insert(contact.to_string());
        self.hub.handle(self.owner_sub, Op::Put { ns: self.ns_for(contact), key: key.to_string(), val: rec.encode() }).notify
    }

    /// Stop sharing a photo with a contact.
    pub fn revoke(&mut self, contact: &str, key: &str) -> Vec<(SubId, Op)> {
        if let Some(set) = self.grants.get_mut(key) { set.remove(contact); }
        self.hub.handle(self.owner_sub, Op::Delete { ns: self.ns_for(contact), key: key.to_string() }).notify
    }

    /// Who a photo is shared with, sorted.
    pub fn shared_with(&self, key: &str) -> Vec<String> {
        let mut v: Vec<String> = self.grants.get(key).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        v.sort();
        v
    }

    pub fn is_shared_with(&self, key: &str, contact: &str) -> bool {
        self.grants.get(key).map(|s| s.contains(contact)).unwrap_or(false)
    }

    /// Everything shared with a contact, by key, as they would see it.
    pub fn visible_to(&self, contact: &str) -> Vec<(String, PhotoRecord)> {
        let mut out: Vec<(String, PhotoRecord)> = self.hub.snapshot(&self.ns_for(contact))
            .into_iter()
            .filter_map(|(k, v)| PhotoRecord::decode(&v).map(|r| (k, r)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// How many photos are shared with anyone.
    pub fn shared_count(&self) -> usize {
        self.grants.values().filter(|s| !s.is_empty()).count()
    }

    /// The grant table as text, one line per grant, for persisting
    /// beside the index. Values are re-derived from the library on load.
    pub fn render_grants(&self) -> String {
        let mut lines: Vec<String> = self.grants.iter()
            .flat_map(|(k, cs)| cs.iter().map(move |c| format!("{c}\t{k}")))
            .collect();
        lines.sort();
        lines.join("\n")
    }

    /// Grants from `render_grants` text. The records are looked up by key
    /// through `lookup`, so a photo no longer in the library drops out.
    pub fn restore_grants(&mut self, text: &str, lookup: impl Fn(&str) -> Option<PhotoRecord>) -> usize {
        let mut n = 0;
        for line in text.lines() {
            let Some((contact, key)) = line.split_once('\t') else { continue };
            if let Some(rec) = lookup(key) {
                self.share_with(contact, key, &rec);
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str) -> PhotoRecord {
        PhotoRecord { name: name.into(), size: 1234, dhash: 0xdead_beef, tags: vec!["beach".into(), "family".into()], at: 1_700_000_000 }
    }

    #[test]
    fn a_record_round_trips_and_garbage_does_not() {
        let r = rec("beach.jpg");
        assert_eq!(PhotoRecord::decode(&r.encode()), Some(r.clone()));
        let none = PhotoRecord { tags: vec![], ..r };
        assert_eq!(PhotoRecord::decode(&none.encode()), Some(none));
        assert_eq!(PhotoRecord::decode("garbage"), None);
    }

    #[test]
    fn each_contact_sees_only_what_was_shared_with_them() {
        let mut s = PhotoShare::new("sc-me");
        s.share_with("sc-ada", "/p/a.jpg", &rec("a.jpg"));
        s.share_with("sc-ada", "/p/b.jpg", &rec("b.jpg"));
        s.share_with("sc-bee", "/p/b.jpg", &rec("b.jpg"));
        assert_eq!(s.visible_to("sc-ada").len(), 2);
        assert_eq!(s.visible_to("sc-bee").len(), 1);
        assert_eq!(s.visible_to("sc-nobody").len(), 0);
        assert_eq!(s.shared_with("/p/b.jpg"), vec!["sc-ada", "sc-bee"]);
        assert_eq!(s.shared_count(), 2);
    }

    #[test]
    fn a_revoke_removes_it_from_their_view_like_stop_sharing() {
        let mut s = PhotoShare::new("sc-me");
        s.share_with("sc-ada", "/p/a.jpg", &rec("a.jpg"));
        assert!(s.is_shared_with("/p/a.jpg", "sc-ada"));
        s.revoke("sc-ada", "/p/a.jpg");
        assert!(!s.is_shared_with("/p/a.jpg", "sc-ada"));
        assert!(s.visible_to("sc-ada").is_empty());
        assert_eq!(s.shared_count(), 0);
    }

    #[test]
    fn grants_persist_as_text_and_come_back_through_the_library() {
        let mut s = PhotoShare::new("sc-me");
        s.share_with("sc-ada", "/p/a.jpg", &rec("a.jpg"));
        s.share_with("sc-bee", "/p/gone.jpg", &rec("gone.jpg"));
        let text = s.render_grants();
        assert_eq!(text, "sc-ada\t/p/a.jpg\nsc-bee\t/p/gone.jpg");
        let mut back = PhotoShare::new("sc-me");
        let n = back.restore_grants(&text, |k| if k == "/p/a.jpg" { Some(rec("a.jpg")) } else { None });
        assert_eq!(n, 1, "a photo no longer in the library drops its grant");
        assert!(back.is_shared_with("/p/a.jpg", "sc-ada"));
        assert!(!back.is_shared_with("/p/gone.jpg", "sc-bee"));
    }

    #[test]
    fn snapclones_contact_file_parses_oldest_first_with_labels() {
        let text = "# snapclone contacts\n\n[sc-2dd464a4]\nnickname = Bee\nport = 41235\nadded_ms = 5\n\n[sc-11111111]\nnickname =\nadded_ms = 1\n\n[not-a-contact]\nnickname = x\n";
        let cs = parse_contacts(text);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].sc_id, "sc-11111111");
        assert_eq!(cs[0].label(), "sc-11111111", "no nickname: the id is the label");
        assert_eq!(cs[1].label(), "Bee");
    }
}
//! share — who may see which photos.
//!
//! The same model `weather-share` gives ambient-learner, because it is the
//! right one for photos too: sharing is **per photo, per contact, and
//! revocable**. Each contact has a private namespace in a
//! [`datashare_core::Hub`] — `ph:{owner}:{contact}` — and a share is a
//! `Put` of the photo's record there, a revoke a `Delete`. A contact sees
//! exactly what was shared with them and nothing else; two contacts never
//! see each other's grants.
//!
//! ## What travels
//!
//! A [`PhotoRecord`]: name, size, hash, tags, when. The BYTES do not go
//! through the hub — a hub value is a string and a photo is megabytes.
//! Delivery of the file itself is snapclone's snap transport, which this
//! app does not link; what this app can do today is keep the grant table
//! that says what the transport is allowed to carry, and it says so on
//! the page rather than showing a "sent" that never happened.
//!
//! ## Contacts
//!
//! snapclone's contact book, read from its own file (`contacts.conf`, one
//! INI section per sc-ID) with `config-kit`. Read only: this app never
//! writes a contact, and never depends on the snapclone crate to read one.

use datashare_core::{Hub, Op, SubId};
use std::collections::{HashMap, HashSet};

/// What a contact learns about a shared photo.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoRecord {
    pub name: String,
    pub size: u64,
    pub dhash: u64,
    /// Written tags only. The derived ones describe the pixels the
    /// recipient will have anyway.
    pub tags: Vec<String>,
    /// Unix seconds the photo was last modified.
    pub at: i64,
}

impl PhotoRecord {
    /// Unit-separated, like `WeatherRecord`, so it round-trips through
    /// the hub's string values. Tags are comma-joined; a comma in a tag
    /// was never allowed by `Library::add_tag`'s callers anyway.
    pub fn encode(&self) -> String {
        format!("{}\u{1f}{}\u{1f}{:016x}\u{1f}{}\u{1f}{}",
                self.name, self.size, self.dhash, self.tags.join(","), self.at)
    }

    pub fn decode(s: &str) -> Option<PhotoRecord> {
        let mut p = s.split('\u{1f}');
        let name = p.next()?.to_string();
        let size = p.next()?.parse().ok()?;
        let dhash = u64::from_str_radix(p.next()?, 16).ok()?;
        let tags: Vec<String> = p.next()?.split(',').filter(|t| !t.is_empty()).map(String::from).collect();
        let at = p.next()?.parse().ok()?;
        Some(PhotoRecord { name, size, dhash, tags, at })
    }
}

/// A snapclone contact, as much of it as sharing needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub sc_id: String,
    pub nickname: String,
}

impl Contact {
    /// The name to show: the nickname, or the id when there is none.
    pub fn label(&self) -> &str {
        if self.nickname.trim().is_empty() { &self.sc_id } else { &self.nickname }
    }
}

/// Parse snapclone's `contacts.conf`.
pub fn parse_contacts(text: &str) -> Vec<Contact> {
    let ini = config_kit::Ini::parse(text);
    let mut out: Vec<(u64, Contact)> = ini.sections.keys()
        .filter(|s| s.starts_with("sc-"))
        .map(|s| {
            let added: u64 = ini.get(s, "added_ms").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
            (added, Contact { sc_id: s.clone(), nickname: ini.get(s, "nickname").unwrap_or("").trim().to_string() })
        })
        .collect();
    // Oldest first, as snapclone lists them.
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sc_id.cmp(&b.1.sc_id)));
    out.into_iter().map(|(_, c)| c).collect()
}

/// snapclone's contacts on this machine, if it has any.
pub fn load_contacts() -> Vec<Contact> {
    config_kit::load("snapclone", "contacts.conf").map(|t| parse_contacts(&t)).unwrap_or_default()
}

/// The owner's exposed photo DB and the grants over it.
pub struct PhotoShare {
    hub: Hub,
    owner: String,
    owner_sub: SubId,
    /// photo key (its path) -> contacts it is shared with.
    grants: HashMap<String, HashSet<String>>,
}

impl PhotoShare {
    pub fn new(owner: impl Into<String>) -> Self {
        PhotoShare { hub: Hub::new(), owner: owner.into(), owner_sub: 1, grants: HashMap::new() }
    }

    fn ns_for(&self, contact: &str) -> String { format!("ph:{}:{}", self.owner, contact) }

    /// Share a photo with a contact. Idempotent.
    pub fn share_with(&mut self, contact: &str, key: &str, rec: &PhotoRecord) -> Vec<(SubId, Op)> {
        self.grants.entry(key.to_string()).or_default().insert(contact.to_string());
        self.hub.handle(self.owner_sub, Op::Put { ns: self.ns_for(contact), key: key.to_string(), val: rec.encode() }).notify
    }

    /// Stop sharing a photo with a contact.
    pub fn revoke(&mut self, contact: &str, key: &str) -> Vec<(SubId, Op)> {
        if let Some(set) = self.grants.get_mut(key) { set.remove(contact); }
        self.hub.handle(self.owner_sub, Op::Delete { ns: self.ns_for(contact), key: key.to_string() }).notify
    }

    /// Who a photo is shared with, sorted.
    pub fn shared_with(&self, key: &str) -> Vec<String> {
        let mut v: Vec<String> = self.grants.get(key).map(|s| s.iter().cloned().collect()).unwrap_or_default();
        v.sort();
        v
    }

    pub fn is_shared_with(&self, key: &str, contact: &str) -> bool {
        self.grants.get(key).map(|s| s.contains(contact)).unwrap_or(false)
    }

    /// Everything shared with a contact, by key, as they would see it.
    pub fn visible_to(&self, contact: &str) -> Vec<(String, PhotoRecord)> {
        let mut out: Vec<(String, PhotoRecord)> = self.hub.snapshot(&self.ns_for(contact))
            .into_iter()
            .filter_map(|(k, v)| PhotoRecord::decode(&v).map(|r| (k, r)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// How many photos are shared with anyone.
    pub fn shared_count(&self) -> usize {
        self.grants.values().filter(|s| !s.is_empty()).count()
    }

    /// The grant table as text, one line per grant, for persisting
    /// beside the index. Values are re-derived from the library on load.
    pub fn render_grants(&self) -> String {
        let mut lines: Vec<String> = self.grants.iter()
            .flat_map(|(k, cs)| cs.iter().map(move |c| format!("{c}\t{k}")))
            .collect();
        lines.sort();
        lines.join("\n")
    }

    /// Grants from `render_grants` text. The records are looked up by key
    /// through `lookup`, so a photo no longer in the library drops out.
    pub fn restore_grants(&mut self, text: &str, lookup: impl Fn(&str) -> Option<PhotoRecord>) -> usize {
        let mut n = 0;
        for line in text.lines() {
            let Some((contact, key)) = line.split_once('\t') else { continue };
            if let Some(rec) = lookup(key) {
                self.share_with(contact, key, &rec);
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str) -> PhotoRecord {
        PhotoRecord { name: name.into(), size: 1234, dhash: 0xdead_beef, tags: vec!["beach".into(), "family".into()], at: 1_700_000_000 }
    }

    #[test]
    fn a_record_round_trips_and_garbage_does_not() {
        let r = rec("beach.jpg");
        assert_eq!(PhotoRecord::decode(&r.encode()), Some(r.clone()));
        let none = PhotoRecord { tags: vec![], ..r };
        assert_eq!(PhotoRecord::decode(&none.encode()), Some(none));
        assert_eq!(PhotoRecord::decode("garbage"), None);
    }

    #[test]
    fn each_contact_sees_only_what_was_shared_with_them() {
        let mut s = PhotoShare::new("sc-me");
        s.share_with("sc-ada", "/p/a.jpg", &rec("a.jpg"));
        s.share_with("sc-ada", "/p/b.jpg", &rec("b.jpg"));
        s.share_with("sc-bee", "/p/b.jpg", &rec("b.jpg"));
        assert_eq!(s.visible_to("sc-ada").len(), 2);
        assert_eq!(s.visible_to("sc-bee").len(), 1);
        assert_eq!(s.visible_to("sc-nobody").len(), 0);
        assert_eq!(s.shared_with("/p/b.jpg"), vec!["sc-ada", "sc-bee"]);
        assert_eq!(s.shared_count(), 2);
    }

    #[test]
    fn a_revoke_removes_it_from_their_view_like_stop_sharing() {
        let mut s = PhotoShare::new("sc-me");
        s.share_with("sc-ada", "/p/a.jpg", &rec("a.jpg"));
        assert!(s.is_shared_with("/p/a.jpg", "sc-ada"));
        s.revoke("sc-ada", "/p/a.jpg");
        assert!(!s.is_shared_with("/p/a.jpg", "sc-ada"));
        assert!(s.visible_to("sc-ada").is_empty());
        assert_eq!(s.shared_count(), 0);
    }

    #[test]
    fn grants_persist_as_text_and_come_back_through_the_library() {
        let mut s = PhotoShare::new("sc-me");
        s.share_with("sc-ada", "/p/a.jpg", &rec("a.jpg"));
        s.share_with("sc-bee", "/p/gone.jpg", &rec("gone.jpg"));
        let text = s.render_grants();
        assert_eq!(text, "sc-ada\t/p/a.jpg\nsc-bee\t/p/gone.jpg");
        let mut back = PhotoShare::new("sc-me");
        let n = back.restore_grants(&text, |k| if k == "/p/a.jpg" { Some(rec("a.jpg")) } else { None });
        assert_eq!(n, 1, "a photo no longer in the library drops its grant");
        assert!(back.is_shared_with("/p/a.jpg", "sc-ada"));
        assert!(!back.is_shared_with("/p/gone.jpg", "sc-bee"));
    }

    #[test]
    fn snapclones_contact_file_parses_oldest_first_with_labels() {
        let text = "# snapclone contacts\n\n[sc-2dd464a4]\nnickname = Bee\nport = 41235\nadded_ms = 5\n\n[sc-11111111]\nnickname =\nadded_ms = 1\n\n[not-a-contact]\nnickname = x\n";
        let cs = parse_contacts(text);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].sc_id, "sc-11111111");
        assert_eq!(cs[0].label(), "sc-11111111", "no nickname: the id is the label");
        assert_eq!(cs[1].label(), "Bee");
    }
}
