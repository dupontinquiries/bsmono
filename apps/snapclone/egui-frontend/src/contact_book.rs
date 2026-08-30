//! contact_book — the contacts that survive a restart, and the port that used
//! to be dropped on the floor.
//!
//! Two separate holes, one module, because they are the same hole seen twice.
//!
//! **Contacts did not persist.** The binary constructed the app from three
//! hardcoded names — `kitchen-tablet`, `studio-mbp`, `porch-cam` — and nothing
//! read or wrote a contact anywhere. Scan someone's code, and they are gone
//! next launch. That is not a missing feature so much as the reason the app
//! read as a demo: the one thing you do on the Friends page had no effect that
//! outlasted the process.
//!
//! **The port was being discarded.** `apply_scan` builds
//! `friends::AddPeer { host, port }` from the scanned code, and `apply_friends`
//! reads `p.host` and never touches `p.port` — so `sc-aabbccdd:9000` and
//! `sc-aabbccdd` produced identical state. The QR parser goes to some trouble
//! over that port (a bare id means 9876, an explicit one overrides) and every
//! bit of it was thrown away one call later.
//!
//! ## Why this is beside `peers` and not merged into it
//!
//! `Snapclone::peers` is `Peer { name, online }` — a DISPLAY row. Online is a
//! live fact that must not be persisted (a contact who was up when you quit is
//! not up now, and writing that down produces a roster of lies on launch), and
//! there is nowhere on it for a port or an added-at. Widening `Peer` would
//! touch thirty-eight literals across six files for a field two of them care
//! about.
//!
//! So the book holds what is durable and `peers` holds what is shown. That IS
//! parallel state and the risk is real — the answer is that `apply_friends` is
//! the only writer of either, and `adding_a_contact_reaches_both_the_book_and_
//! the_display_row` fails if a future change updates one. Merging them is the
//! right end state and this is not it; the seam is at least in one place.

/// One person, as durably known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    /// `sc-xxxxxxxx`. The identity, and the key.
    pub sc_id: String,
    /// A local label. Local because this app cannot rename someone on their own
    /// device, and two people disagreeing about who they are talking to is
    /// worse than a bare id.
    pub nickname: String,
    /// Where to reach them. Defaulted from the QR parser's rule rather than
    /// left optional, so no caller has to re-derive 9876.
    pub port: u16,
    /// Unix ms. Kept for ordering: a list that reshuffles between launches is
    /// one people stop trusting their eyes on.
    pub added_ms: u64,
    /// Their Ed25519 key, pinned the first time a signed announcement from them
    /// verified. `None` until then — a contact added by QR or by hand has an
    /// sc-ID and nothing else.
    ///
    /// **This is what makes the sc-ID safe.** The id is four bytes of the key's
    /// hash, so grinding a keypair whose fingerprint matches a chosen sc-ID is
    /// about 2^32 work — minutes. Pinning removes the payoff: once a real key
    /// is on file for that id, a ground one no longer matches and is refused.
    /// Trust on first use, with everything that implies: the FIRST sighting is
    /// the one that cannot be checked.
    pub public: Option<[u8; 32]>,
}

impl Contact {
    /// What to show. Never blank — a row with no label cannot be told from
    /// another row with no label.
    pub fn label(&self) -> &str {
        let n = self.nickname.trim();
        if n.is_empty() { &self.sc_id } else { n }
    }
}

/// The book. Ordered by insertion, which after a load means ordered by
/// `added_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContactBook {
    entries: Vec<Contact>,
}

impl ContactBook {
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn iter(&self) -> std::slice::Iter<'_, Contact> { self.entries.iter() }
    pub fn get(&self, sc_id: &str) -> Option<&Contact> {
        self.entries.iter().find(|c| c.sc_id == sc_id)
    }

    /// Add, or update the port of someone already known.
    ///
    /// Re-scanning a code must not create a second row for the same person, and
    /// must not silently reset a nickname the user typed — the code carries no
    /// name, so treating a rescan as new data about the label would erase it.
    /// The port DOES update, because that is the one thing a rescan can
    /// legitimately be telling you.
    ///
    /// Returns whether anything changed, so a caller can avoid writing a file
    /// for a no-op.
    pub fn upsert(&mut self, sc_id: &str, port: u16, added_ms: u64) -> bool {
        if let Some(c) = self.entries.iter_mut().find(|c| c.sc_id == sc_id) {
            if c.port == port { return false; }
            c.port = port;
            return true;
        }
        self.entries.push(Contact {
            sc_id: sc_id.to_string(), nickname: String::new(), port, added_ms, public: None,
        });
        true
    }

    /// Set a local nickname. An empty one clears it back to the bare id rather
    /// than being refused, because "remove the label I gave them" is a thing
    /// people mean.
    pub fn rename(&mut self, sc_id: &str, to: &str) -> bool {
        match self.entries.iter_mut().find(|c| c.sc_id == sc_id) {
            Some(c) if c.nickname != to.trim() => { c.nickname = to.trim().to_string(); true }
            _ => false,
        }
    }

    /// Record or check a contact's key.
    ///
    /// Not a setter: the whole value is in refusing the third case.
    pub fn pin(&mut self, sc_id: &str, public: [u8; 32]) -> Pin {
        let Some(c) = self.entries.iter_mut().find(|c| c.sc_id == sc_id) else {
            // Not a contact. Nothing to pin against and nowhere to keep it —
            // pinning strangers would mean writing the address book every time
            // an unknown device on the subnet said hello.
            return Pin::Unknown;
        };
        match c.public {
            None => { c.public = Some(public); Pin::First }
            Some(k) if k == public => Pin::Same,
            Some(_) => Pin::Changed,
        }
    }

    pub fn remove(&mut self, sc_id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|c| c.sc_id != sc_id);
        self.entries.len() != before
    }

    /// The display rows this book implies, all offline.
    ///
    /// Offline is not a placeholder pending better information — it is the only
    /// honest state at load. Presence is learned from gossip after the network
    /// comes up, and a roster that showed everyone green on launch would be
    /// showing yesterday's news as today's.
    pub fn to_peers(&self) -> Vec<crate::Peer> {
        self.entries.iter()
            .map(|c| crate::Peer { name: c.label().to_string(), online: false })
            .collect()
    }
}

/// What happened when a key was checked against a contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pin {
    /// First signed sighting; the key is now on file.
    First,
    /// Matches what we had.
    Same,
    /// A DIFFERENT key for an id we have a key for.
    ///
    /// Either they reinstalled, or somebody ground a keypair to collide with
    /// their sc-ID. Those are indistinguishable from here, which is exactly why
    /// this is refused and surfaced rather than silently re-pinned: silently
    /// taking the new key makes the pin worthless, and silently dropping the
    /// datagram makes a legitimate reinstall look like a broken network.
    Changed,
    /// Not in the address book at all.
    Unknown,
}

/// The file, beside `settings.conf`.
pub const FILE: &str = "contacts.conf";

/// Serialise.
///
/// One section per contact, keyed by sc-ID, rather than one section with
/// numbered keys: the id in the header is what a person scanning this file is
/// looking for, and a numbered scheme renumbers everyone below a deletion.
pub fn render(book: &ContactBook) -> String {
    let mut out = String::from("# snapclone contacts. One section per person, keyed by sc-ID.\n");
    for c in book.iter() {
        out.push_str(&format!("\n[{}]\n", c.sc_id));
        out.push_str(&format!("nickname = {}\n", c.nickname));
        out.push_str(&format!("port = {}\n", c.port));
        out.push_str(&format!("added_ms = {}\n", c.added_ms));
        if let Some(k) = c.public {
            out.push_str(&format!("public = {}\n",
                                  k.iter().map(|b| format!("{b:02x}")).collect::<String>()));
        }
    }
    out
}

/// Parse, skipping entries that are not usable.
///
/// A section whose name is not a valid sc-ID is dropped: the id is the routing
/// key, and keeping a contact nobody can be reached at produces a row that
/// fails silently every time it is tapped. A bad `port` line, by contrast,
/// keeps the contact and takes the default — losing a person over a typo in
/// one field is the harsher failure.
pub fn parse(text: &str) -> ContactBook {
    let ini = config_kit::Ini::parse(text);
    let mut book = ContactBook::default();
    for sc_id in ini.sections.keys() {
        let sc_id = sc_id.as_str();
        if !crate::identity::is_valid(sc_id) { continue; }
        let port = ini.get(sc_id, "port").and_then(|v| v.trim().parse().ok())
            .unwrap_or(crate::qr::DEFAULT_PORT);
        let added_ms = ini.get(sc_id, "added_ms").and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        // A malformed key is dropped rather than kept: half a pin is worse than
        // none, because the next verified sighting would read as a key CHANGE
        // and refuse a peer who did nothing wrong.
        let public = ini.get(sc_id, "public").and_then(|v| {
            let v = v.trim();
            if v.len() != 64 { return None; }
            let mut out = [0u8; 32];
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = u8::from_str_radix(v.get(i * 2..i * 2 + 2)?, 16).ok()?;
            }
            Some(out)
        });
        book.entries.push(Contact {
            sc_id: sc_id.to_string(),
            nickname: ini.get(sc_id, "nickname").unwrap_or("").trim().to_string(),
            port,
            added_ms,
            public,
        });
    }
    // Oldest first, so the list a user sees does not reshuffle when the file's
    // section order changes for reasons that have nothing to do with them.
    book.entries.sort_by_key(|c| c.added_ms);
    book
}

pub fn load() -> ContactBook {
    config_kit::load(crate::settings_store::APP, FILE)
        .map(|t| parse(&t))
        .unwrap_or_default()
}

pub fn store(book: &ContactBook) -> std::io::Result<()> {
    config_kit::save(crate::settings_store::APP, FILE, &render(book))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> ContactBook {
        let mut b = ContactBook::default();
        b.upsert("sc-aabbccdd", 9876, 1_000);
        b.upsert("sc-11223344", 9000, 2_000);
        b
    }

    #[test]
    fn a_scanned_port_survives_the_round_trip() {
        // The bug this exists for: apply_friends read AddPeer.host and never
        // touched AddPeer.port, so sc-aabbccdd:9000 and sc-aabbccdd produced
        // identical state and the parser's care over the default was wasted.
        let got = parse(&render(&book()));
        assert_eq!(got.get("sc-11223344").map(|c| c.port), Some(9000));
        assert_eq!(got.get("sc-aabbccdd").map(|c| c.port), Some(9876));
    }

    #[test]
    fn everything_round_trips_including_a_nickname_with_spaces() {
        let mut b = book();
        b.rename("sc-aabbccdd", "Ada  Lovelace");
        assert_eq!(parse(&render(&b)), b);
    }

    #[test]
    fn rescanning_a_code_updates_the_port_without_duplicating_or_renaming() {
        // Three separate mistakes a naive add would make. The nickname one
        // matters most: the code carries no name, so treating a rescan as new
        // label data erases something only the user could have supplied.
        let mut b = book();
        b.rename("sc-aabbccdd", "Ada");
        assert!(b.upsert("sc-aabbccdd", 9100, 9_999));
        assert_eq!(b.len(), 2, "a rescan made a second row");
        assert_eq!(b.get("sc-aabbccdd").unwrap().nickname, "Ada", "the label was erased");
        assert_eq!(b.get("sc-aabbccdd").unwrap().port, 9100);
        assert_eq!(b.get("sc-aabbccdd").unwrap().added_ms, 1_000, "and it is not newly added");
    }

    #[test]
    fn an_unchanged_rescan_reports_no_change_so_no_file_is_written() {
        let mut b = book();
        assert!(!b.upsert("sc-aabbccdd", 9876, 5_000));
        assert!(!b.rename("sc-aabbccdd", ""), "already blank");
    }

    #[test]
    fn a_blank_nickname_clears_the_label_rather_than_being_refused() {
        // "Remove the label I gave them" is a thing people mean, and the row
        // falls back to the id rather than becoming unidentifiable.
        let mut b = book();
        b.rename("sc-aabbccdd", "Ada");
        assert!(b.rename("sc-aabbccdd", "   "));
        assert_eq!(b.get("sc-aabbccdd").unwrap().label(), "sc-aabbccdd");
    }

    #[test]
    fn a_section_that_is_not_an_sc_id_is_dropped_but_a_bad_port_is_not() {
        // Asymmetric on purpose: the id is the routing key, so a contact
        // without a good one is a row that fails silently every time it is
        // tapped. A typo'd port costs a default, not a person.
        let b = parse("[not-an-id]\nport = 9000\n\n[sc-aabbccdd]\nport = banana\n");
        assert_eq!(b.len(), 1);
        assert_eq!(b.get("sc-aabbccdd").unwrap().port, crate::qr::DEFAULT_PORT);
    }

    #[test]
    fn contacts_come_back_oldest_first_whatever_order_the_file_is_in() {
        // A list that reshuffles between launches is one people stop trusting
        // their eyes on.
        let b = parse("[sc-11223344]\nadded_ms = 2000\n\n[sc-aabbccdd]\nadded_ms = 1000\n");
        let ids: Vec<&str> = b.iter().map(|c| c.sc_id.as_str()).collect();
        assert_eq!(ids, vec!["sc-aabbccdd", "sc-11223344"]);
    }

    #[test]
    fn an_absent_file_is_an_empty_book_not_an_error() {
        assert_eq!(parse(""), ContactBook::default());
        assert!(parse("# just a comment\n").is_empty());
    }

    #[test]
    fn loaded_contacts_are_all_offline() {
        // Presence is learned from gossip after the network is up. A roster
        // that showed everyone green on launch would be showing yesterday's
        // news as today's, and the Connect button would lie about which of
        // them is reachable.
        let rows = book().to_peers();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|p| !p.online));
    }

    #[test]
    fn a_display_row_uses_the_nickname_when_there_is_one() {
        let mut b = book();
        b.rename("sc-aabbccdd", "Ada");
        let names: Vec<String> = b.to_peers().into_iter().map(|p| p.name).collect();
        assert!(names.contains(&"Ada".to_string()));
        assert!(names.contains(&"sc-11223344".to_string()), "and the bare id otherwise");
    }

    #[test]
    fn a_key_is_pinned_on_the_first_signed_sighting_and_matches_after() {
        let mut b = book();
        assert_eq!(b.pin("sc-aabbccdd", [7u8; 32]), Pin::First);
        assert_eq!(b.pin("sc-aabbccdd", [7u8; 32]), Pin::Same);
        assert_eq!(b.get("sc-aabbccdd").unwrap().public, Some([7u8; 32]));
    }

    #[test]
    fn a_different_key_for_a_pinned_id_is_refused_rather_than_re_pinned() {
        // The attack the pin exists for: the sc-ID is four bytes of the key's
        // hash, so grinding a keypair to collide with a chosen id is about 2^32
        // work. Silently taking the new key would make the pin worthless.
        let mut b = book();
        b.pin("sc-aabbccdd", [7u8; 32]);
        assert_eq!(b.pin("sc-aabbccdd", [8u8; 32]), Pin::Changed);
        assert_eq!(b.get("sc-aabbccdd").unwrap().public, Some([7u8; 32]),
                   "the stored key was overwritten by the challenger");
    }

    #[test]
    fn a_stranger_is_not_pinned_because_there_is_nowhere_to_keep_it() {
        // Pinning unknown devices would write the address book every time
        // anything on the subnet said hello.
        let mut b = book();
        assert_eq!(b.pin("sc-99999999", [7u8; 32]), Pin::Unknown);
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn a_pinned_key_survives_the_file_and_a_broken_one_does_not_half_survive() {
        let mut b = book();
        b.pin("sc-aabbccdd", [7u8; 32]);
        assert_eq!(parse(&render(&b)), b);
        // Half a pin is worse than none: the next verified sighting would read
        // as a key CHANGE and refuse a peer who did nothing wrong.
        let broken = parse("[sc-aabbccdd]\nport = 9876\npublic = beef\n");
        assert_eq!(broken.get("sc-aabbccdd").unwrap().public, None);
    }

    #[test]
    fn removing_someone_removes_them() {
        let mut b = book();
        assert!(b.remove("sc-aabbccdd"));
        assert!(!b.remove("sc-aabbccdd"), "and says so the second time");
        assert_eq!(b.len(), 1);
    }
}
