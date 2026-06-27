//! Query parser. Grammar (future-proofed; only Range/Bucket implemented):
//!   path             -> whole file
//!   path:N           -> single line N
//!   path:A-B         -> inclusive range
//!   path:5xx         -> 500..=599   (bucket, width 100)
//!   path:12xx        -> 1200..=1299
//!   path:5k          -> 5000..=5999 (bucket, width 1000)
//!   path:@symbol     -> reserved (not yet implemented)
use crate::query::Query;
use anyhow::{bail, Result};
use std::path::PathBuf;

pub fn parse_query(input: &str) -> Result<Query> {
    let colon_pos = match input.rfind(':') {
        Some(i) => i,
        // No range → the whole file.
        None => {
            return Ok(Query::Range {
                path: PathBuf::from(input),
                start: 1,
                end: usize::MAX,
            })
        }
    };
    let (path_str, rest) = input.split_at(colon_pos);
    if path_str.is_empty() {
        bail!("Invalid format: empty path in '{input}'");
    }
    let range_str = &rest[1..]; // drop ':'
    if range_str.is_empty() {
        return Ok(Query::Range { path: PathBuf::from(path_str), start: 1, end: usize::MAX });
    }
    if range_str.starts_with('@') {
        bail!("Symbol queries reserved (not yet implemented)");
    }
    let path = PathBuf::from(path_str);

    // A-B explicit range
    if let Some(dash) = range_str.find('-') {
        let s = parse_num(&range_str[..dash])?.unwrap_or(1);
        let e = parse_num(&range_str[dash + 1..])?.unwrap_or(usize::MAX);
        return Ok(Query::Range { path, start: s.max(1), end: e });
    }
    // Nxx bucket (width 100)
    if let Some(stripped) = range_str.strip_suffix("xx") {
        let b = parse_num(stripped)?.unwrap_or(0);
        return Ok(Query::Bucket { path, bucket: b, width: 100 });
    }
    // Nk bucket (width 1000) — bucket index is N, NOT N*1000
    if let Some(stripped) = range_str.strip_suffix('k') {
        let b = parse_num(stripped)?.unwrap_or(0);
        return Ok(Query::Bucket { path, bucket: b, width: 1000 });
    }
    if range_str.ends_with('m') {
        bail!("Million buckets reserved");
    }
    // Single line N
    if let Some(n) = parse_num(range_str)? {
        return Ok(Query::Range { path, start: n.max(1), end: n });
    }
    bail!("Invalid range: '{range_str}'");
}

fn parse_num(s: &str) -> Result<Option<usize>> {
    if s.is_empty() {
        return Ok(None);
    }
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.is_empty() {
        return Ok(None);
    }
    Ok(Some(d.parse()?))
}
