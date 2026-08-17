//! audit_log.rs — an append-only, tamper-evident record of things that must survive the agent.
//!
//! WHY THIS EXISTS
//!
//! `taint::Anomaly` currently reaches the model and the UI and then evaporates. That means a
//! poisoned shared doc gets defanged silently, on every future run, forever — the injection sits in
//! the repo working on every model you ever point at it, and nothing ever tells you. Neutralisation
//! without a record is worse than it looks: it converts a discoverable problem into a permanent
//! invisible one.
//!
//! The same log is the substrate for capability grants (which GPU lease, when, for how long) and
//! for the review gate. Those all share one property: the agent must be able to CAUSE entries and
//! must not be able to REWRITE them.
//!
//! WHAT TAMPER-EVIDENT MEANS HERE, PRECISELY
//!
//! Each entry carries the hash of the previous one, so editing or removing an entry in the middle
//! breaks every hash after it and `verify()` says exactly where. That is detection, not prevention:
//! anyone who can write the file can also rewrite the whole chain from the edit point onward and
//! produce a self-consistent log.
//!
//! The chain is therefore only as good as its ANCHOR — a copy of the head hash kept somewhere the
//! writer cannot reach. `head()` gives you that value; commit it to the root-owned git mirror, or
//! print it to a terminal you keep. Without an anchor, TRUNCATION is undetectable by construction:
//! a log with the last N entries removed is a perfectly valid shorter log. `verify_against_anchor`
//! is the only check that catches it, and it needs a value you stored elsewhere.
//!
//! SHA-256 is implemented here rather than pulled in, because the crate has no crypto dependency
//! and a hash chain built on a non-cryptographic hash (twox-hash is already in the tree) is
//! forgeable in seconds and would be worse than no chain at all — it would look like evidence.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

// ── SHA-256 ─────────────────────────────────────────────────────────────────────────────────────

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of `data`, lowercase hex. Verified against the NIST vectors in the tests below.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bitlen = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e;
            e = d.wrapping_add(t1);
            d = c; c = b; b = a;
            a = t1.wrapping_add(t2);
        }
        for (i, v) in [a, b, c, d, e, f, g, hh].iter().enumerate() {
            h[i] = h[i].wrapping_add(*v);
        }
    }
    h.iter().map(|x| format!("{:08x}", x)).collect()
}

// ── entries ─────────────────────────────────────────────────────────────────────────────────────

/// What happened. Kept coarse on purpose: a log with fifty entry kinds is a log nobody reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Tool-call syntax rejected, or found in content that should not carry it.
    Injection,
    /// A capability was handed out (GPU device, network egress, a writable path).
    Grant,
    /// A capability ended — expiry, revocation, or teardown.
    Revoke,
    /// A verification run was refused by policy.
    Refusal,
    /// A human looked at accumulated work and accepted it.
    Review,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Injection => "injection",
            Kind::Grant => "grant",
            Kind::Revoke => "revoke",
            Kind::Refusal => "refusal",
            Kind::Review => "review",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "injection" => Kind::Injection,
            "grant" => Kind::Grant,
            "revoke" => Kind::Revoke,
            "refusal" => Kind::Refusal,
            "review" => Kind::Review,
            _ => return None,
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub seq: u64,
    pub unix_time: u64,
    pub kind: Kind,
    /// What it was about: a doc name, a device path, a branch.
    pub subject: String,
    pub detail: String,
    pub prev: String,
    pub hash: String,
}

impl Entry {
    /// The bytes that are hashed. Field order is part of the format; changing it invalidates every
    /// existing chain, which is why it is written out explicitly rather than derived.
    fn preimage(seq: u64, t: u64, kind: Kind, subject: &str, detail: &str, prev: &str) -> String {
        format!("{seq}\u{1f}{t}\u{1f}{}\u{1f}{subject}\u{1f}{detail}\u{1f}{prev}", kind.as_str())
    }

    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.seq,
            self.unix_time,
            self.kind.as_str(),
            escape(&self.subject),
            escape(&self.detail),
            self.prev,
            self.hash
        )
    }

    fn parse(line: &str) -> Option<Entry> {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 {
            return None;
        }
        Some(Entry {
            seq: f[0].parse().ok()?,
            unix_time: f[1].parse().ok()?,
            kind: Kind::parse(f[2])?,
            subject: unescape(f[3]),
            detail: unescape(f[4]),
            prev: f[5].to_string(),
            hash: f[6].to_string(),
        })
    }
}

// Tabs and newlines are the record separators, so they cannot appear in fields. A hostile subject
// is exactly the sort of thing that ends up in here.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

// ── the log ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tamper {
    /// An entry's hash does not match its contents — it was edited in place.
    Rewritten { seq: u64 },
    /// An entry's `prev` does not match the previous entry's hash — something was inserted or removed.
    ChainBroken { seq: u64 },
    /// Sequence numbers are not consecutive.
    SequenceGap { expected: u64, found: u64 },
    /// A line could not be parsed.
    Malformed { line: usize },
    /// The head does not match the anchor you stored elsewhere: entries were removed from the END,
    /// which no amount of internal checking can detect.
    Truncated { expected_head: String, found_head: String },
}

impl fmt::Display for Tamper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tamper::Rewritten { seq } => write!(f, "entry {seq} was edited after it was written"),
            Tamper::ChainBroken { seq } => write!(f, "the chain breaks at entry {seq} — something was inserted or removed before it"),
            Tamper::SequenceGap { expected, found } => write!(f, "expected entry {expected}, found {found}"),
            Tamper::Malformed { line } => write!(f, "line {line} is not a valid entry"),
            Tamper::Truncated { expected_head, found_head } => write!(
                f, "the log ends at {} but your anchor says {} — entries were removed from the end",
                &found_head[..8.min(found_head.len())], &expected_head[..8.min(expected_head.len())]
            ),
        }
    }
}

pub struct AuditLog {
    path: PathBuf,
}

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

impl AuditLog {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> Vec<Entry> {
        let Ok(txt) = std::fs::read_to_string(&self.path) else { return Vec::new() };
        txt.lines().filter(|l| !l.trim().is_empty()).filter_map(Entry::parse).collect()
    }

    /// The value to publish out of band. Anchor it or truncation is invisible.
    pub fn head(&self) -> String {
        self.entries().last().map(|e| e.hash.clone()).unwrap_or_else(|| GENESIS.to_string())
    }

    pub fn append(&self, kind: Kind, subject: &str, detail: &str) -> std::io::Result<Entry> {
        let existing = self.entries();
        let seq = existing.last().map(|e| e.seq + 1).unwrap_or(0);
        let prev = existing.last().map(|e| e.hash.clone()).unwrap_or_else(|| GENESIS.to_string());
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let subject = escape_free(subject);
        let detail = escape_free(detail);
        let hash = sha256_hex(Entry::preimage(seq, t, kind, &subject, &detail, &prev).as_bytes());
        let e = Entry { seq, unix_time: t, kind, subject, detail, prev, hash };

        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(f, "{}", e.line())?;
        f.flush()?;
        Ok(e)
    }

    /// Walk the chain. Catches edits, insertions, and removals anywhere but the end.
    pub fn verify(&self) -> Result<usize, Tamper> {
        let Ok(txt) = std::fs::read_to_string(&self.path) else { return Ok(0) };
        let mut prev = GENESIS.to_string();
        let mut expect_seq = 0u64;
        let mut n = 0usize;
        for (i, line) in txt.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let e = Entry::parse(line).ok_or(Tamper::Malformed { line: i + 1 })?;
            if e.seq != expect_seq {
                return Err(Tamper::SequenceGap { expected: expect_seq, found: e.seq });
            }
            if e.prev != prev {
                return Err(Tamper::ChainBroken { seq: e.seq });
            }
            let want = sha256_hex(
                Entry::preimage(e.seq, e.unix_time, e.kind, &e.subject, &e.detail, &e.prev).as_bytes(),
            );
            if want != e.hash {
                return Err(Tamper::Rewritten { seq: e.seq });
            }
            prev = e.hash;
            expect_seq += 1;
            n += 1;
        }
        Ok(n)
    }

    /// The only check that catches truncation. `anchor` is a `head()` you stored somewhere the
    /// writer of this file cannot reach.
    pub fn verify_against_anchor(&self, anchor: &str) -> Result<usize, Tamper> {
        let n = self.verify()?;
        // An anchor from the past is satisfied if it appears anywhere in the current chain: the log
        // is allowed to have GROWN since you anchored it, just not to have shrunk.
        if anchor == GENESIS {
            return Ok(n);
        }
        if self.entries().iter().any(|e| e.hash == anchor) {
            Ok(n)
        } else {
            Err(Tamper::Truncated { expected_head: anchor.to_string(), found_head: self.head() })
        }
    }
}

/// Field separators cannot survive in a field. Done at write time so a malicious subject cannot
/// forge extra columns.
fn escape_free(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).take(4096).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ls-audit-{name}-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    // ── the hash has to actually be SHA-256, or the chain is decoration ─────────────────────────

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_handles_a_multi_block_input() {
        // 1,000,000 'a' — the classic long vector, which exercises the padding path across blocks.
        let data = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&data),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    // ── the log ────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn entries_chain_and_verify() {
        let log = AuditLog::at(tmp("chain"));
        log.append(Kind::Injection, "notes.md", "tool call in shared doc").unwrap();
        log.append(Kind::Refusal, "verify", "no bwrap").unwrap();
        log.append(Kind::Grant, "/dev/dri/renderD128", "lease 10m").unwrap();
        assert_eq!(log.verify(), Ok(3));
        let e = log.entries();
        assert_eq!(e[1].prev, e[0].hash);
        assert_eq!(e[2].prev, e[1].hash);
        let _ = std::fs::remove_file(log.path());
    }

    #[test]
    fn editing_an_entry_is_detected() {
        let p = tmp("edit");
        let log = AuditLog::at(&p);
        log.append(Kind::Injection, "a", "one").unwrap();
        log.append(Kind::Injection, "b", "two").unwrap();
        let txt = std::fs::read_to_string(&p).unwrap().replace("one", "ONE");
        std::fs::write(&p, txt).unwrap();
        assert_eq!(log.verify(), Err(Tamper::Rewritten { seq: 0 }));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn removing_a_middle_entry_is_detected() {
        let p = tmp("remove");
        let log = AuditLog::at(&p);
        for i in 0..3 {
            log.append(Kind::Injection, &format!("doc{i}"), "x").unwrap();
        }
        let txt = std::fs::read_to_string(&p).unwrap();
        let kept: Vec<&str> = txt.lines().enumerate().filter(|(i, _)| *i != 1).map(|(_, l)| l).collect();
        std::fs::write(&p, kept.join("\n") + "\n").unwrap();
        // The sequence check fires first, which is the more legible error of the two.
        assert_eq!(log.verify(), Err(Tamper::SequenceGap { expected: 1, found: 2 }));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_forged_entry_with_a_recomputed_hash_still_breaks_the_chain() {
        let p = tmp("forge");
        let log = AuditLog::at(&p);
        log.append(Kind::Injection, "a", "one").unwrap();
        log.append(Kind::Injection, "b", "two").unwrap();
        // Attacker rewrites entry 0 AND fixes its own hash, but cannot fix entry 1's `prev`
        // without rewriting the whole tail.
        let mut e = log.entries();
        e[0].detail = "harmless".into();
        e[0].hash = sha256_hex(
            Entry::preimage(e[0].seq, e[0].unix_time, e[0].kind, &e[0].subject, &e[0].detail, &e[0].prev)
                .as_bytes(),
        );
        std::fs::write(&p, format!("{}\n{}\n", e[0].line(), e[1].line())).unwrap();
        assert_eq!(log.verify(), Err(Tamper::ChainBroken { seq: 1 }));
        let _ = std::fs::remove_file(&p);
    }

    /// The honest limit: internal verification cannot see a log that simply stops early.
    #[test]
    fn truncation_is_invisible_without_an_anchor() {
        let p = tmp("trunc");
        let log = AuditLog::at(&p);
        log.append(Kind::Injection, "a", "one").unwrap();
        let anchor = log.head();
        log.append(Kind::Injection, "b", "two").unwrap();
        log.append(Kind::Grant, "gpu", "lease").unwrap();
        let anchor_after = log.head();

        // Roll back to one entry.
        let txt = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, format!("{}\n", txt.lines().next().unwrap())).unwrap();

        assert_eq!(log.verify(), Ok(1), "a truncated log is internally consistent");
        assert!(log.verify_against_anchor(&anchor).is_ok(), "an older anchor is still satisfied");
        match log.verify_against_anchor(&anchor_after) {
            Err(Tamper::Truncated { .. }) => {}
            other => panic!("anchor must catch truncation, got {:?}", other),
        }
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_hostile_subject_cannot_forge_extra_columns() {
        let p = tmp("inject");
        let log = AuditLog::at(&p);
        log.append(Kind::Injection, "evil\t99\treview\tfake\tapproved", "x\nmore").unwrap();
        assert_eq!(log.verify(), Ok(1), "still one entry, not two");
        let e = log.entries();
        assert_eq!(e.len(), 1);
        assert!(!e[0].subject.contains('\t'));
        assert!(!e[0].detail.contains('\n'));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn an_empty_or_missing_log_verifies_as_zero() {
        let log = AuditLog::at(tmp("missing"));
        assert_eq!(log.verify(), Ok(0));
        assert_eq!(log.head(), GENESIS);
    }

    #[test]
    fn appends_survive_a_reopen() {
        let p = tmp("reopen");
        AuditLog::at(&p).append(Kind::Review, "main", "read 400 lines").unwrap();
        let again = AuditLog::at(&p);
        again.append(Kind::Review, "main", "read 20 more").unwrap();
        assert_eq!(again.verify(), Ok(2));
        assert_eq!(again.entries()[1].seq, 1);
        let _ = std::fs::remove_file(&p);
    }
}
