//! # syncdiff — what's duplicated where, and what has drifted apart
//!
//! Works entirely on stored catalogs, so the drives DON'T need to be
//! plugged in: "your tax folder exists on RedSticker and BlueCase, and
//! BlueCase is missing 14 files that RedSticker has" is answerable from
//! the drawer. Two analyses:
//!
//! 1. Duplicates: files appearing on multiple devices, grouped by
//!    (file name, size) — deliberately loose (no content hash required)
//!    because default scans don't hash; when both sides carry a BLAKE3,
//!    it upgrades the match to Confirmed. Loose matches are labeled
//!    Probable, never presented as certainty.
//! 2. Backup-set desync: top-level directories sharing a name across
//!    two devices are treated as intended replicas ("docs" on two
//!    drives is a backup relationship until told otherwise), and the
//!    pair is diffed: present only on A, only on B, or present on both
//!    with differing size/mtime/hash — with byte totals so the operator
//!    sees the COST of each gap, not just a count.

use crate::catalog::Catalog;
use crate::integrity::Entry;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum MatchGrade {
    /// Same name + size; contents not verified (default scans don't hash).
    Probable,
    /// BLAKE3 present on both sides and equal.
    Confirmed,
}

#[derive(Clone, Debug, Serialize)]
pub struct DupGroup {
    pub file_name: String,
    pub size: u64,
    pub grade: MatchGrade,
    /// (device key, rel path) for every occurrence.
    pub locations: Vec<(String, String)>,
}

/// Cross-device duplicate report. Groups by (file_name, size); a group
/// qualifies when it spans ≥2 DEVICES (many copies on one drive is a
/// cleanup question, not a "what's duplicated where" question).
pub fn duplicates(cats: &[&Catalog]) -> Vec<DupGroup> {
    let mut by_key: BTreeMap<(String, u64), Vec<(usize, &Entry)>> = BTreeMap::new();
    for (ci, c) in cats.iter().enumerate() {
        for e in c.entries.values() {
            let name = e.path.rsplit('/').next().unwrap_or(&e.path).to_string();
            by_key.entry((name, e.size)).or_default().push((ci, e));
        }
    }
    let mut out = Vec::new();
    for ((name, size), occ) in by_key {
        let mut devs: Vec<usize> = occ.iter().map(|(ci, _)| *ci).collect();
        devs.sort_unstable();
        devs.dedup();
        if devs.len() < 2 {
            continue;
        }
        let hashes: Vec<&str> = occ.iter().map(|(_, e)| e.b3.as_str()).collect();
        let grade = if hashes.iter().all(|h| !h.is_empty())
            && hashes.windows(2).all(|w| w[0] == w[1])
        {
            MatchGrade::Confirmed
        } else {
            MatchGrade::Probable
        };
        out.push(DupGroup {
            file_name: name,
            size,
            grade,
            locations: occ
                .iter()
                .map(|(ci, e)| (cats[*ci].meta.identity.key(), e.path.clone()))
                .collect(),
        });
    }
    // Biggest savings first.
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

#[derive(Clone, Debug, Serialize)]
pub struct DesyncReport {
    pub dir: String,
    pub dev_a: String,
    pub dev_b: String,
    /// rel paths (within the shared dir) present only on A, with sizes.
    pub only_a: Vec<(String, u64)>,
    pub only_b: Vec<(String, u64)>,
    /// present on both but size/mtime/hash differ: (rel, size_a, size_b,
    /// newer_side: 'a'|'b'|'?').
    pub differing: Vec<(String, u64, u64, char)>,
    pub only_a_bytes: u64,
    pub only_b_bytes: u64,
}

impl DesyncReport {
    pub fn in_sync(&self) -> bool {
        self.only_a.is_empty() && self.only_b.is_empty() && self.differing.is_empty()
    }
}

/// Diff every top-level directory the two catalogs share by name.
pub fn desync(a: &Catalog, b: &Catalog) -> Vec<DesyncReport> {
    let tops = |c: &Catalog| -> Vec<String> {
        let mut t: Vec<String> = c
            .entries
            .keys()
            .filter_map(|p| p.split_once('/').map(|(top, _)| top.to_string()))
            .collect();
        t.sort();
        t.dedup();
        t
    };
    let ta = tops(a);
    let tb = tops(b);
    let mut out = Vec::new();
    for dir in ta.iter().filter(|d| tb.contains(d)) {
        let prefix = format!("{dir}/");
        let sub = |c: &Catalog| -> BTreeMap<String, Entry> {
            c.entries
                .iter()
                .filter(|(p, _)| p.starts_with(&prefix))
                .map(|(p, e)| (p[prefix.len()..].to_string(), e.clone()))
                .collect()
        };
        let sa = sub(a);
        let sb = sub(b);
        let mut rep = DesyncReport {
            dir: dir.clone(),
            dev_a: a.meta.identity.key(),
            dev_b: b.meta.identity.key(),
            only_a: Vec::new(),
            only_b: Vec::new(),
            differing: Vec::new(),
            only_a_bytes: 0,
            only_b_bytes: 0,
        };
        for (rel, ea) in &sa {
            match sb.get(rel) {
                None => {
                    rep.only_a_bytes += ea.size;
                    rep.only_a.push((rel.clone(), ea.size));
                }
                Some(eb) => {
                    let same_hash =
                        !ea.b3.is_empty() && !eb.b3.is_empty() && ea.b3 == eb.b3;
                    if !same_hash && (ea.size != eb.size || ea.mtime_unix != eb.mtime_unix) {
                        let newer = if ea.mtime_unix > eb.mtime_unix {
                            'a'
                        } else if eb.mtime_unix > ea.mtime_unix {
                            'b'
                        } else {
                            '?'
                        };
                        rep.differing.push((rel.clone(), ea.size, eb.size, newer));
                    }
                }
            }
        }
        for (rel, eb) in &sb {
            if !sa.contains_key(rel) {
                rep.only_b_bytes += eb.size;
                rep.only_b.push((rel.clone(), eb.size));
            }
        }
        // Big gaps first within each list.
        rep.only_a.sort_by(|x, y| y.1.cmp(&x.1));
        rep.only_b.sort_by(|x, y| y.1.cmp(&x.1));
        out.push(rep);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, CatalogMeta, DeviceIdentity};

    fn cat(key: &str, files: &[(&str, u64, i64)]) -> Catalog {
        let entries = files
            .iter()
            .map(|(p, sz, mt)| {
                (
                    p.to_string(),
                    Entry { path: p.to_string(), size: *sz, mtime_unix: *mt, b3: String::new() },
                )
            })
            .collect();
        Catalog {
            meta: CatalogMeta {
                identity: DeviceIdentity {
                    fs_uuid: Some(key.into()),
                    label: None,
                    model: None,
                    serial: None,
                },
                taken_at: 0,
                mount_root: String::new(),
                photo_file: None,
                entry_count: 0,
                total_bytes: 0,
            },
            entries,
        }
    }

    #[test]
    fn dupes_and_desync_report_the_gaps() {
        let a = cat(
            "DRV-A",
            &[
                ("docs/tax/2023.pdf", 1000, 100),
                ("docs/tax/2024.pdf", 2000, 200),
                ("docs/cv.docx", 50, 50),
                ("media/big.mkv", 9_000_000, 10),
            ],
        );
        let b = cat(
            "DRV-B",
            &[
                ("docs/tax/2023.pdf", 1000, 100), // dup + in sync
                ("docs/cv.docx", 60, 300),        // differs, b newer
                ("docs/notes.txt", 5, 1),         // only on b
                ("media/big.mkv", 9_000_000, 10), // dup
            ],
        );
        let dups = duplicates(&[&a, &b]);
        assert_eq!(dups.len(), 2);
        assert_eq!(dups[0].file_name, "big.mkv"); // sorted by size desc
        assert_eq!(dups[0].grade, MatchGrade::Probable);
        assert_eq!(dups[0].locations.len(), 2);

        let reps = desync(&a, &b);
        let docs = reps.iter().find(|r| r.dir == "docs").expect("docs report");
        assert_eq!(docs.only_a, vec![("tax/2024.pdf".to_string(), 2000)]);
        assert_eq!(docs.only_b, vec![("notes.txt".to_string(), 5)]);
        assert_eq!(docs.differing.len(), 1);
        assert_eq!(docs.differing[0].3, 'b'); // b is newer
        assert_eq!(docs.only_a_bytes, 2000);
        let media = reps.iter().find(|r| r.dir == "media").expect("media report");
        assert!(media.in_sync());
    }
}
