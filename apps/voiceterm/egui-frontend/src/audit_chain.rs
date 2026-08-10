//! audit_chain — the hash chain and record format from `audit_log.rs`.
//!
//! An append-only log of every command block, each entry referencing the
//! SHA-256 of the one before. The chain is what makes the log *verifiable*
//! rather than merely written: a tampered entry breaks every hash after it.
//!
//! ## Why not a real git repo, in the original's own accounting
//!
//! Three reasons, and they are measurements rather than preferences.
//! Spawning `git commit` per command *"adds 20-50ms latency. At interactive
//! rates (~100 commands/hour during active use) that's seconds/hour of
//! overhead."* It *"adds a hard dep on the `git` binary"*, and a voice shell
//! *"should run on minimal systems where git might be absent."* And the
//! metadata-only case *"doesn't actually benefit from git's tree storage. We're
//! using git purely for its commit chain, which is just a hash-linked list —
//! trivial to implement directly."*
//!
//! That last line is the one worth keeping: the feature needed one property of
//! git, and taking the dependency for it would have bought the other ninety.
//!
//! ## The line format was chosen to be greppable
//!
//! Key=value pairs, one line per record, values escaped so no literal newline,
//! tab or backslash can appear. The stated goals are that it be *"trivially
//! greppable from a terminal"* and *"trivially parseable in any language"* —
//! which for an audit log matters more than density. A log you cannot read with
//! `grep` during an incident is a log you do not read.
//!
//! ## Rotation deliberately breaks the chain
//!
//! *"Chain restarts after rotation — the first entry in the new log has
//! prev_hash empty."* That is honest rather than convenient: the new file
//! genuinely cannot be verified against records it does not contain, and
//! pretending otherwise by carrying the hash forward would make verification
//! report success across a boundary it never checked.

/// Rotate past this size. Old logs are renamed, never deleted.
pub const ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// Escape a value so it cannot break the line format.
///
/// Backslash first: escaping it after the others would double-process the
/// backslashes those escapes just introduced, turning `\n` into `\\n`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

/// Reverse [`escape`].
///
/// A trailing lone backslash is kept literally rather than dropped: a truncated
/// write should read back as visibly odd rather than silently shortening the
/// value it ended.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' { out.push(c); continue; }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            // An unknown escape keeps both characters, so an entry written by a
            // newer version reads back intact rather than losing bytes.
            Some(other) => { out.push('\\'); out.push(other); }
            None => out.push('\\'),
        }
    }
    out
}

/// One record's chainable content.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub ts_ms: u64,
    pub command: String,
    pub speaker: Option<String>,
    /// Hash of the previous entry; empty for the first in a file.
    pub prev_hash: String,
}

impl Entry {
    /// The line written to disk.
    pub fn render(&self) -> String {
        let mut s = format!("ts={} cmd={}", self.ts_ms, escape(&self.command));
        if let Some(sp) = &self.speaker {
            s.push_str(&format!(" speaker={}", escape(sp)));
        }
        s.push_str(&format!(" prev={}", self.prev_hash));
        s
    }

    /// The hash the next entry chains against.
    ///
    /// Computed over the RENDERED line, so what is verified is exactly what is
    /// on disk. Hashing the struct instead would let a rendering change break
    /// verification of records written before it, with no field having altered.
    pub fn hash(&self) -> String { sha256_hex(self.render().as_bytes()) }
}

/// A chain of entries in one file.
#[derive(Clone, Debug, Default)]
pub struct Chain {
    tail: String,
    len: usize,
}

impl Chain {
    pub fn tail(&self) -> &str { &self.tail }
    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Chain an entry, filling in its `prev_hash`.
    ///
    /// Takes the entry rather than a builder because the hash must be computed
    /// AFTER prev_hash is set — an entry hashed before chaining would verify
    /// against a line that was never written.
    pub fn append(&mut self, mut e: Entry) -> Entry {
        e.prev_hash = self.tail.clone();
        self.tail = e.hash();
        self.len += 1;
        e
    }

    /// Start a new file after rotation.
    ///
    /// Clears the tail deliberately: the new file cannot be verified against
    /// records it does not contain, and carrying the hash forward would have
    /// verification report success across a boundary it never checked.
    pub fn rotate(&mut self) {
        self.tail.clear();
        self.len = 0;
    }

    pub fn should_rotate(&self, bytes: u64) -> bool { bytes >= ROTATE_BYTES }
}

/// Verify a run of entries.
///
/// Returns the index of the first broken link, or `None` when intact.
pub fn verify(entries: &[Entry]) -> Option<usize> {
    let mut expected = String::new();
    for (i, e) in entries.iter().enumerate() {
        if e.prev_hash != expected { return Some(i); }
        expected = e.hash();
    }
    None
}

/// Minimal SHA-256, so the audit chain has no dependency of its own.
///
/// The point of the log is that it works on a minimal system — taking a crate
/// for a hash would reintroduce exactly the dependency argument that ruled out
/// git.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
        0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                           0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bitlen.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1],
                                       chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(t1);
            d = c; c = b; b = a; a = t1.wrapping_add(t2);
        }
        for (i, v) in [a, b, c, d, e, f, g, hh].iter().enumerate() {
            h[i] = h[i].wrapping_add(*v);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(cmd: &str) -> Entry {
        Entry { ts_ms: 1_000, command: cmd.into(), speaker: None, prev_hash: String::new() }
    }

    #[test]
    fn the_hash_matches_the_reference_vectors() {
        // A hand-written SHA-256 that is subtly wrong would produce a chain
        // that verifies against itself and nothing else — so it is checked
        // against the published vectors rather than against its own output.
        assert_eq!(sha256_hex(b""),
                   "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"),
                   "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn a_chain_verifies_when_intact() {
        let mut c = Chain::default();
        let a = c.append(entry("ls"));
        let b = c.append(entry("cd /tmp"));
        assert_eq!(a.prev_hash, "", "the first entry has no predecessor");
        assert_eq!(b.prev_hash, a.hash());
        assert_eq!(verify(&[a, b]), None);
    }

    #[test]
    fn tampering_with_an_entry_breaks_every_link_after_it() {
        // The property the chain exists for: an edited record cannot be made to
        // fit without rewriting everything that followed.
        let mut c = Chain::default();
        let a = c.append(entry("ls"));
        let b = c.append(entry("cd /tmp"));
        let mut tampered = a.clone();
        tampered.command = "rm -rf /".into();
        assert_eq!(verify(&[tampered, b]), Some(1), "the NEXT entry is where it shows");
    }

    #[test]
    fn deleting_an_entry_is_detected() {
        // Removing a record leaves a gap the following prev_hash cannot match.
        let mut c = Chain::default();
        let a = c.append(entry("ls"));
        let _b = c.append(entry("secret"));
        let d = c.append(entry("exit"));
        assert_eq!(verify(&[a, d]), Some(1));
    }

    #[test]
    fn the_hash_covers_the_rendered_line_not_the_struct() {
        // What is verified must be exactly what is on disk. Hashing the struct
        // would let a rendering change break verification of older records with
        // no field having altered.
        let e = entry("ls -la");
        assert_eq!(e.hash(), sha256_hex(e.render().as_bytes()));
    }

    #[test]
    fn a_newline_in_a_command_cannot_forge_a_second_record() {
        // The attack the escaping prevents: a command containing a newline and
        // a plausible ts= would otherwise appear as two entries, one of them
        // unchained.
        let mut e = entry("ls\nts=999 cmd=rm prev=");
        e.speaker = Some("attacker".into());
        let line = e.render();
        assert_eq!(line.lines().count(), 1, "still one line: {line}");
        assert!(line.contains("\\n"));
    }

    #[test]
    fn escaping_round_trips_including_backslashes() {
        // Backslash must be escaped FIRST, or escaping it afterwards
        // double-processes the ones the other escapes just introduced.
        for s in ["plain", "a\\b", "tab\there", "nl\nthere", "cr\rhere",
                  "\\n literal", "\\", "mixed\\\t\n"] {
            assert_eq!(unescape(&escape(s)), s, "{s:?}");
        }
    }

    #[test]
    fn an_unknown_escape_keeps_both_characters() {
        // An entry written by a newer version reads back intact rather than
        // losing bytes — an audit log that quietly drops data is worse than one
        // that shows something it does not understand.
        assert_eq!(unescape("a\\qb"), "a\\qb");
    }

    #[test]
    fn a_truncated_line_ending_in_a_backslash_is_read_literally() {
        // A partial write should look visibly odd rather than silently
        // shortening the value it ended.
        assert_eq!(unescape("abc\\"), "abc\\");
    }

    #[test]
    fn rotation_restarts_the_chain_rather_than_pretending_to_continue() {
        // "The first entry in the new log has prev_hash empty." Carrying the
        // hash forward would have verification report success across a boundary
        // it never checked.
        let mut c = Chain::default();
        c.append(entry("ls"));
        assert!(!c.tail().is_empty());
        c.rotate();
        assert_eq!(c.tail(), "");
        let first = c.append(entry("after rotation"));
        assert_eq!(first.prev_hash, "");
        assert_eq!(verify(&[first]), None, "the new file verifies on its own");
    }

    #[test]
    fn rotation_triggers_at_the_size_the_original_uses() {
        let c = Chain::default();
        assert!(!c.should_rotate(ROTATE_BYTES - 1));
        assert!(c.should_rotate(ROTATE_BYTES));
        assert_eq!(ROTATE_BYTES, 10 * 1024 * 1024);
    }

    #[test]
    fn an_empty_log_verifies() {
        // A fresh install has no entries, and reporting a break there would
        // teach the user to ignore the check.
        assert_eq!(verify(&[]), None);
        assert!(Chain::default().is_empty());
    }

    #[test]
    fn a_speaker_field_is_escaped_too() {
        // Attribution is attacker-influenced in a multi-speaker session.
        let mut e = entry("ls");
        e.speaker = Some("bob\nts=0 cmd=x".into());
        assert_eq!(e.render().lines().count(), 1);
    }
}
