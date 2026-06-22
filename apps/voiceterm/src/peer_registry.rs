//! v0.1.715: peer registry. Each received peer lives under
//! `<notes_root>/peers/<author>/` with a `MANIFEST.web2qml` (author, digest,
//! files, shared_at) and, once paired, a `.voiceprint` file (CSV embedding).
//! This module is the PURE parsing/rendering core: turn a peer's directory
//! facts into a PeerRecord and render the browser list. The fs scan + git live
//! in main.rs.

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeerRecord {
    /// directory name under peers/ (the author slug).
    pub dir: String,
    /// author handle from the manifest (falls back to dir).
    pub author: String,
    /// workspace digest from the manifest, "" if unknown.
    pub digest: String,
    /// file count from the manifest, 0 if unknown.
    pub files: u32,
    /// shared_at unix ts from the manifest, 0 if unknown.
    pub shared_at: i64,
    /// a stored voiceprint is on file (peer is voice-verifiable).
    pub has_voiceprint: bool,
}

/// Parse a MANIFEST.web2qml body (`key = value` lines) into the manifest fields
/// of a PeerRecord. `dir` is the directory name; `has_voiceprint` is supplied
/// by the caller (it knows whether the .voiceprint file exists).
pub fn parse_record(dir: &str, manifest_body: &str, has_voiceprint: bool) -> PeerRecord {
    let mut rec = PeerRecord {
        dir: dir.to_string(),
        author: dir.to_string(),
        has_voiceprint,
        ..Default::default()
    };
    for line in manifest_body.lines() {
        let mut it = line.splitn(2, '=');
        let key = it.next().map(|s| s.trim()).unwrap_or("");
        let val = it.next().map(|s| s.trim()).unwrap_or("");
        match key {
            "author" if !val.is_empty() => rec.author = val.to_string(),
            "digest" => rec.digest = val.to_string(),
            "files" => rec.files = val.parse().unwrap_or(0),
            "shared_at" => rec.shared_at = val.parse().unwrap_or(0),
            _ => {}
        }
    }
    rec
}

/// Render a list of records to a wire for QML: one peer per line,
/// `dir\tauthor\tdigest\tfiles\tshared_at\thas_voiceprint(0/1)`.
pub fn list_wire(records: &[PeerRecord]) -> String {
    records.iter().map(|r| format!("{}\t{}\t{}\t{}\t{}\t{}",
        r.dir, r.author, if r.digest.is_empty() { "-" } else { &r.digest },
        r.files, r.shared_at, if r.has_voiceprint { 1 } else { 0 }))
        .collect::<Vec<_>>().join("\n")
}

/// Validate that a CSV voiceprint wire is a usable embedding of `expect_dim`
/// floats (so we don't store junk).
pub fn valid_voiceprint(wire: &str, expect_dim: usize) -> bool {
    let n = wire.split(',').filter(|t| !t.trim().is_empty())
        .filter(|t| t.trim().parse::<f32>().is_ok()).count();
    n == expect_dim && expect_dim > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest() {
        let body = "author = Alice Cooper\ndigest = a8c8ac4ebb3f167f\nfiles = 12\nshared_at = 1700000000\n";
        let r = parse_record("alice-cooper", body, true);
        assert_eq!(r.author, "Alice Cooper");
        assert_eq!(r.digest, "a8c8ac4ebb3f167f");
        assert_eq!(r.files, 12);
        assert_eq!(r.shared_at, 1700000000);
        assert!(r.has_voiceprint);
        assert_eq!(r.dir, "alice-cooper");
    }

    #[test]
    fn missing_fields_fall_back() {
        let r = parse_record("bob", "", false);
        assert_eq!(r.author, "bob");       // falls back to dir
        assert_eq!(r.digest, "");
        assert_eq!(r.files, 0);
        assert!(!r.has_voiceprint);
    }

    #[test]
    fn list_wire_shape() {
        let recs = vec![
            parse_record("alice", "author = alice\ndigest = abc\nfiles = 3\nshared_at = 100\n", true),
            parse_record("bob", "", false),
        ];
        let w = list_wire(&recs);
        let lines: Vec<&str> = w.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "alice\talice\tabc\t3\t100\t1");
        assert_eq!(lines[1], "bob\tbob\t-\t0\t0\t0");
    }

    #[test]
    fn voiceprint_validation() {
        assert!(valid_voiceprint("0.1,0.2,0.3", 3));
        assert!(!valid_voiceprint("0.1,0.2", 3));     // wrong dim
        assert!(!valid_voiceprint("", 3));            // empty
        assert!(!valid_voiceprint("a,b,c", 3));       // not floats
        assert!(!valid_voiceprint("0.1,0.2,0.3", 0)); // zero expect
    }
}
