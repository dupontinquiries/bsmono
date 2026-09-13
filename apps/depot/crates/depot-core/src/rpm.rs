//! Fedora/RHEL RPMs: read the header, write dnf's repodata.
//!
//! An .rpm is four regions back to back:
//!
//! ```text
//! lead        96 bytes, magic ed ab ee db — legacy, only the magic matters
//! signature   a "header" structure (below), padded to 8 bytes; carries the
//!             payload size and the GPG signature dnf checks at install time
//! header      the real metadata: name, version, deps, file list, changelog
//! payload     compressed cpio archive (zstd on modern Fedora); never read here
//! ```
//!
//! A header structure is: 8 magic bytes (`8e ad e8 01` + 4 reserved), u32
//! entry count, u32 data size, then `count` 16-byte index entries (tag, type,
//! offset, count) followed by the data blob they point into. All big-endian.
//! That is the whole format; the difficulty is entirely in WHICH tags dnf
//! wants and how createrepo_c filters them, so the tests here diff depot's
//! output against createrepo_c's over four reference packages (see
//! testdata/README.md) rather than trusting the description above.
//!
//! What dnf reads from a repo:
//!
//! ```text
//! repodata/repomd.xml        the index: each metadata file, its checksum, size
//! repodata/primary.xml.gz    per package: NEVRA, summary, deps, "primary" files
//! repodata/filelists.xml.gz  per package: every file (for `dnf provides /path`)
//! repodata/other.xml.gz      per package: changelog
//! Packages/…/name.rpm        the artifacts, at the <location href> primary gives
//! ```
//!
//! depot writes each package's three XML fragments ONCE at ingest and stores
//! them as objects; a repo rebuild is then a concatenation, and the fragments
//! ride along in packs like any other object. createrepo_c's `.sqlite.bz2`
//! files are not produced — dnf stopped reading them years ago.

use crate::{invalid, sha256_file, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

// ── tags (rpmtag.h) ──────────────────────────────────────────────────────────
const NAME: u32 = 1000;
const VERSION: u32 = 1001;
const RELEASE: u32 = 1002;
const EPOCH: u32 = 1003;
const SUMMARY: u32 = 1004;
const DESCRIPTION: u32 = 1005;
const BUILDTIME: u32 = 1006;
const BUILDHOST: u32 = 1007;
const SIZE: u32 = 1009;
const VENDOR: u32 = 1011;
const LICENSE: u32 = 1014;
const PACKAGER: u32 = 1015;
const GROUP: u32 = 1016;
const URL: u32 = 1020;
const ARCH: u32 = 1022;
const FILEMODES: u32 = 1030;
const FILEFLAGS: u32 = 1037;
const SOURCERPM: u32 = 1044;
const ARCHIVESIZE: u32 = 1046;
const PROVIDENAME: u32 = 1047;
const REQUIREFLAGS: u32 = 1048;
const REQUIRENAME: u32 = 1049;
const REQUIREVERSION: u32 = 1050;
const CONFLICTFLAGS: u32 = 1053;
const CONFLICTNAME: u32 = 1054;
const CONFLICTVERSION: u32 = 1055;
const CHANGELOGTIME: u32 = 1080;
const CHANGELOGNAME: u32 = 1081;
const CHANGELOGTEXT: u32 = 1082;
const OBSOLETENAME: u32 = 1090;
const SOURCEPACKAGE: u32 = 1106;
const PROVIDEFLAGS: u32 = 1112;
const PROVIDEVERSION: u32 = 1113;
const OBSOLETEFLAGS: u32 = 1114;
const OBSOLETEVERSION: u32 = 1115;
const DIRINDEXES: u32 = 1116;
const BASENAMES: u32 = 1117;
const DIRNAMES: u32 = 1118;
const LONGSIZE: u32 = 5009;
const RECOMMENDNAME: u32 = 5046;
const RECOMMENDVERSION: u32 = 5047;
const RECOMMENDFLAGS: u32 = 5048;
const SUGGESTNAME: u32 = 5049;
const SUGGESTVERSION: u32 = 5050;
const SUGGESTFLAGS: u32 = 5051;
const SUPPLEMENTNAME: u32 = 5052;
const SUPPLEMENTVERSION: u32 = 5053;
const SUPPLEMENTFLAGS: u32 = 5054;
const ENHANCENAME: u32 = 5055;
const ENHANCEVERSION: u32 = 5056;
const ENHANCEFLAGS: u32 = 5057;
// signature header
const SIG_PAYLOADSIZE: u32 = 1007;
const SIG_LONGARCHIVESIZE: u32 = 271;

// ── dependency flags (rpmds.h) ───────────────────────────────────────────────
const SENSE_LESS: u32 = 1 << 1;
const SENSE_GREATER: u32 = 1 << 2;
const SENSE_EQUAL: u32 = 1 << 3;
const SENSE_PREREQ: u32 = 1 << 6;
const SENSE_SCRIPT_PRE: u32 = 1 << 9;
const SENSE_SCRIPT_POST: u32 = 1 << 10;
const SENSE_RPMLIB: u32 = 1 << 24;
const FILE_GHOST: u32 = 1 << 6;
const S_IFMT: u16 = 0o170000;
const S_IFDIR: u16 = 0o040000;

// ─────────────────────────────────────────────────────────────────────────────
// Header structure
// ─────────────────────────────────────────────────────────────────────────────

struct Entry {
    tag: u32,
    ty: u32,
    offset: u32,
    count: u32,
}

struct Header {
    entries: Vec<Entry>,
    data: Vec<u8>,
}

impl Header {
    /// Parse one header structure at the reader's current position; returns
    /// it and the number of bytes it occupied (unpadded).
    fn read(r: &mut impl Read) -> Result<(Header, u64)> {
        let mut pre = [0u8; 16];
        r.read_exact(&mut pre)?;
        if pre[..3] != [0x8e, 0xad, 0xe8] {
            return Err(invalid("bad header magic"));
        }
        if pre[3] != 1 {
            return Err(invalid(format!("unsupported header version {}", pre[3])));
        }
        let n = u32::from_be_bytes([pre[8], pre[9], pre[10], pre[11]]) as usize;
        let size = u32::from_be_bytes([pre[12], pre[13], pre[14], pre[15]]) as usize;
        if n > 65536 || size > 256 * 1024 * 1024 {
            return Err(invalid(format!("implausible header: {n} entries, {size} bytes")));
        }
        let mut idx = vec![0u8; n * 16];
        r.read_exact(&mut idx)?;
        let mut data = vec![0u8; size];
        r.read_exact(&mut data)?;
        let entries = idx
            .chunks_exact(16)
            .map(|c| Entry {
                tag: u32::from_be_bytes([c[0], c[1], c[2], c[3]]),
                ty: u32::from_be_bytes([c[4], c[5], c[6], c[7]]),
                offset: u32::from_be_bytes([c[8], c[9], c[10], c[11]]),
                count: u32::from_be_bytes([c[12], c[13], c[14], c[15]]),
            })
            .collect();
        Ok((Header { entries, data }, 16 + (n * 16) as u64 + size as u64))
    }

    fn find(&self, tag: u32) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }

    /// STRING (6), or the first locale of an I18NSTRING (9).
    fn string(&self, tag: u32) -> Option<String> {
        let e = self.find(tag)?;
        if e.ty != 6 && e.ty != 9 {
            return None;
        }
        self.cstr_at(e.offset as usize)
    }

    fn str_or_empty(&self, tag: u32) -> String {
        self.string(tag).unwrap_or_default()
    }

    /// STRING_ARRAY (8) or I18NSTRING (9): `count` consecutive C strings.
    fn strings(&self, tag: u32) -> Vec<String> {
        let Some(e) = self.find(tag) else { return Vec::new() };
        if e.ty != 8 && e.ty != 9 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(e.count as usize);
        let mut off = e.offset as usize;
        for _ in 0..e.count {
            match self.cstr_at(off) {
                Some(s) => {
                    off += s.len() + 1;
                    out.push(s);
                }
                None => break,
            }
        }
        out
    }

    fn cstr_at(&self, off: usize) -> Option<String> {
        let rest = self.data.get(off..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    fn u32s(&self, tag: u32) -> Vec<u32> {
        let Some(e) = self.find(tag) else { return Vec::new() };
        if e.ty != 4 {
            return Vec::new();
        }
        let off = e.offset as usize;
        (0..e.count as usize)
            .filter_map(|i| self.data.get(off + i * 4..off + i * 4 + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]])))
            .collect()
    }

    fn u16s(&self, tag: u32) -> Vec<u16> {
        let Some(e) = self.find(tag) else { return Vec::new() };
        if e.ty != 3 {
            return Vec::new();
        }
        let off = e.offset as usize;
        (0..e.count as usize).filter_map(|i| self.data.get(off + i * 2..off + i * 2 + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))).collect()
    }

    fn u64_scalar(&self, tag: u32) -> Option<u64> {
        let e = self.find(tag)?;
        let off = e.offset as usize;
        match e.ty {
            4 => self.data.get(off..off + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64),
            5 => self.data.get(off..off + 8).map(|b| u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// What we extract
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    /// "EQ" | "LT" | "GT" | "LE" | "GE" | "" (unversioned)
    pub flags: String,
    pub epoch: Option<String>,
    pub ver: Option<String>,
    pub rel: Option<String>,
    pub pre: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    /// "" | "dir" | "ghost"
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Changelog {
    pub author: String,
    pub date: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpmInfo {
    pub name: String,
    pub epoch: u64,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub is_source: bool,
    pub summary: String,
    pub description: String,
    pub packager: String,
    pub url: String,
    pub build_time: u64,
    pub size_installed: u64,
    pub size_archive: u64,
    pub size_package: u64,
    pub license: String,
    pub vendor: String,
    pub group: String,
    pub buildhost: String,
    pub sourcerpm: String,
    pub header_start: u64,
    pub header_end: u64,
    pub sha256: String,
    pub provides: Vec<Dep>,
    pub requires: Vec<Dep>,
    pub conflicts: Vec<Dep>,
    pub obsoletes: Vec<Dep>,
    pub suggests: Vec<Dep>,
    pub enhances: Vec<Dep>,
    pub recommends: Vec<Dep>,
    pub supplements: Vec<Dep>,
    pub files: Vec<FileEntry>,
    /// Oldest first, at most `CHANGELOG_LIMIT` newest entries.
    pub changelog: Vec<Changelog>,
}

pub const CHANGELOG_LIMIT: usize = 10;

impl RpmInfo {
    pub fn nevra(&self) -> String {
        if self.epoch > 0 {
            format!("{}-{}:{}-{}.{}", self.name, self.epoch, self.version, self.release, self.arch)
        } else {
            format!("{}-{}-{}.{}", self.name, self.version, self.release, self.arch)
        }
    }
    /// The conventional filename: `name-version-release.arch.rpm` (epoch is
    /// never in the filename).
    pub fn filename(&self) -> String {
        format!("{}-{}-{}.{}.rpm", self.name, self.version, self.release, if self.is_source { "src".to_string() } else { self.arch.clone() })
    }
}

/// Read an RPM's lead, signature header and header. The payload is not read
/// (but the whole file is hashed for the pkgid).
pub fn read_rpm(path: &Path) -> Result<RpmInfo> {
    let (sha256, size_package) = sha256_file(path)?;
    read_rpm_known(path, &sha256, size_package)
}

/// `read_rpm` for a file whose sha256 and size are already known (an object
/// in the store): only the headers are read.
pub fn read_rpm_known(path: &Path, sha256: &str, size_package: u64) -> Result<RpmInfo> {
    let sha256 = sha256.to_string();
    let mut f = File::open(path)?;
    let mut lead = [0u8; 96];
    f.read_exact(&mut lead).map_err(|_| invalid(format!("{}: too short for an RPM lead", path.display())))?;
    if lead[..4] != [0xed, 0xab, 0xee, 0xdb] {
        return Err(invalid(format!("{}: not an RPM (bad lead magic)", path.display())));
    }
    let (sig, sig_len) = Header::read(&mut f).map_err(|e| invalid(format!("{}: signature header: {e}", path.display())))?;
    // The signature header is padded to an 8-byte boundary; the main header
    // starts right after the padding. That offset is what primary.xml calls
    // header-range start (dnf can fetch just the header with a byte Range).
    let header_start = 96 + (sig_len + 7) / 8 * 8;
    f.seek(SeekFrom::Start(header_start))?;
    let (h, h_len) = Header::read(&mut f).map_err(|e| invalid(format!("{}: header: {e}", path.display())))?;
    let header_end = header_start + h_len;

    let name = h.string(NAME).ok_or_else(|| invalid(format!("{}: header has no NAME", path.display())))?;
    let version = h.str_or_empty(VERSION);
    let release = h.str_or_empty(RELEASE);
    let arch = h.str_or_empty(ARCH);
    let epoch = h.u64_scalar(EPOCH).unwrap_or(0);
    let is_source = h.find(SOURCEPACKAGE).is_some();

    // Files first: requires filtering needs them.
    let dirnames = h.strings(DIRNAMES);
    let basenames = h.strings(BASENAMES);
    let dirindexes = h.u32s(DIRINDEXES);
    let modes = h.u16s(FILEMODES);
    let fflags = h.u32s(FILEFLAGS);
    let mut files = Vec::with_capacity(basenames.len());
    for (i, base) in basenames.iter().enumerate() {
        let dir = dirindexes.get(i).and_then(|&d| dirnames.get(d as usize)).map(String::as_str).unwrap_or("");
        let mode = modes.get(i).copied().unwrap_or(0);
        let fl = fflags.get(i).copied().unwrap_or(0);
        let kind = if fl & FILE_GHOST != 0 {
            "ghost"
        } else if mode & S_IFMT == S_IFDIR {
            "dir"
        } else {
            ""
        };
        files.push(FileEntry { path: format!("{dir}{base}"), kind: kind.into() });
    }
    let file_set: HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();

    let provides = deps(&h, PROVIDENAME, PROVIDEFLAGS, PROVIDEVERSION, false);
    let provided: HashSet<&str> = provides.iter().map(|d| d.name.as_str()).collect();
    let requires = filter_requires(deps(&h, REQUIRENAME, REQUIREFLAGS, REQUIREVERSION, true), &provided, &file_set);

    let mut changelog: Vec<Changelog> = h
        .u32s(CHANGELOGTIME)
        .into_iter()
        .zip(h.strings(CHANGELOGNAME))
        .zip(h.strings(CHANGELOGTEXT))
        .take(CHANGELOG_LIMIT)
        .map(|((t, a), x)| Changelog { author: a, date: t as u64, text: x })
        .collect();
    changelog.reverse(); // header is newest-first; the XML is oldest-first

    let size_archive = sig.u64_scalar(SIG_LONGARCHIVESIZE).or_else(|| sig.u64_scalar(SIG_PAYLOADSIZE)).or_else(|| h.u64_scalar(ARCHIVESIZE)).unwrap_or(0);

    Ok(RpmInfo {
        name,
        epoch,
        version,
        release,
        arch,
        is_source,
        summary: h.str_or_empty(SUMMARY),
        description: h.str_or_empty(DESCRIPTION),
        packager: h.str_or_empty(PACKAGER),
        url: h.str_or_empty(URL),
        build_time: h.u64_scalar(BUILDTIME).unwrap_or(0),
        size_installed: h.u64_scalar(LONGSIZE).or_else(|| h.u64_scalar(SIZE)).unwrap_or(0),
        size_archive,
        size_package,
        license: h.str_or_empty(LICENSE),
        vendor: h.str_or_empty(VENDOR),
        group: h.str_or_empty(GROUP),
        buildhost: h.str_or_empty(BUILDHOST),
        sourcerpm: h.str_or_empty(SOURCERPM),
        header_start,
        header_end,
        sha256,
        provides,
        requires,
        conflicts: deps(&h, CONFLICTNAME, CONFLICTFLAGS, CONFLICTVERSION, false),
        obsoletes: deps(&h, OBSOLETENAME, OBSOLETEFLAGS, OBSOLETEVERSION, false),
        suggests: deps(&h, SUGGESTNAME, SUGGESTFLAGS, SUGGESTVERSION, false),
        enhances: deps(&h, ENHANCENAME, ENHANCEFLAGS, ENHANCEVERSION, false),
        recommends: deps(&h, RECOMMENDNAME, RECOMMENDFLAGS, RECOMMENDVERSION, false),
        supplements: deps(&h, SUPPLEMENTNAME, SUPPLEMENTFLAGS, SUPPLEMENTVERSION, false),
        files,
        changelog,
    })
}

fn deps(h: &Header, name_tag: u32, flags_tag: u32, ver_tag: u32, is_requires: bool) -> Vec<Dep> {
    let names = h.strings(name_tag);
    let flags = h.u32s(flags_tag);
    let vers = h.strings(ver_tag);
    let mut out = Vec::with_capacity(names.len());
    for (i, name) in names.into_iter().enumerate() {
        let fl = flags.get(i).copied().unwrap_or(0);
        let ver = vers.get(i).cloned().unwrap_or_default();
        let flags_s = match (fl & SENSE_LESS != 0, fl & SENSE_GREATER != 0, fl & SENSE_EQUAL != 0) {
            (true, false, true) => "LE",
            (false, true, true) => "GE",
            (true, false, false) => "LT",
            (false, true, false) => "GT",
            (false, false, true) => "EQ",
            _ => "",
        };
        let (epoch, v, rel) = if flags_s.is_empty() {
            (None, None, None)
        } else {
            // "e:v-r": epoch defaults to 0 when a version is present (that is
            // what dnf compares against); rel is absent unless there is a '-'.
            let (e, rest) = match ver.split_once(':') {
                Some((e, r)) => (e.to_string(), r),
                None => ("0".to_string(), ver.as_str()),
            };
            let (v, r) = match rest.split_once('-') {
                Some((v, r)) => (v.to_string(), Some(r.to_string())),
                None => (rest.to_string(), None),
            };
            (Some(e), Some(v), r)
        };
        let pre = is_requires && fl & (SENSE_PREREQ | SENSE_SCRIPT_PRE | SENSE_SCRIPT_POST) != 0;
        let dep = Dep { name, flags: flags_s.into(), epoch, ver: v, rel, pre };
        if is_requires && fl & SENSE_RPMLIB != 0 {
            continue; // rpmlib(...) capabilities are about rpm itself, never a package
        }
        out.push(dep);
    }
    out
}

/// createrepo_c's require filtering, replicated rule for rule (each rule is
/// pinned by a reference package in the tests):
///  1. drop `rpmlib(...)` (done in `deps`);
///  2. drop a require whose NAME the package itself provides (even if the
///     versions would not match — that is what createrepo_c does);
///  3. drop a file require the package itself ships;
///  4. drop exact duplicates, where `pre` is part of the identity;
///  5. of all `libc.so.6*` requires keep only the highest GLIBC version, and
///     append it LAST. (A glibc symbol-version require implies every lower
///     one, so one entry is enough, and the position is an artefact of how
///     createrepo_c defers it.)
fn filter_requires(reqs: Vec<Dep>, provided: &HashSet<&str>, files: &HashSet<&str>) -> Vec<Dep> {
    let mut out = Vec::with_capacity(reqs.len());
    let mut seen: HashSet<(String, String, String, bool)> = HashSet::new();
    let mut libc: Option<(Vec<u64>, Dep)> = None;
    for d in reqs {
        if provided.contains(d.name.as_str()) {
            continue;
        }
        if d.name.starts_with('/') && files.contains(d.name.as_str()) {
            continue;
        }
        let full_ver = format!("{}:{}-{}", d.epoch.as_deref().unwrap_or(""), d.ver.as_deref().unwrap_or(""), d.rel.as_deref().unwrap_or(""));
        if !seen.insert((d.name.clone(), d.flags.clone(), full_ver, d.pre)) {
            continue;
        }
        if d.name.starts_with("libc.so.6") {
            let key = glibc_version_key(&d.name);
            match &libc {
                Some((k, _)) if *k >= key => {}
                _ => libc = Some((key, d)),
            }
            continue;
        }
        out.push(d);
    }
    if let Some((_, d)) = libc {
        out.push(d);
    }
    out
}

/// `libc.so.6(GLIBC_2.2.5)(64bit)` → [2, 2, 5]; unversioned → [] (lowest).
fn glibc_version_key(name: &str) -> Vec<u64> {
    name.split("GLIBC_").nth(1).map(|s| s.split(')').next().unwrap_or("").split('.').filter_map(|p| p.parse().ok()).collect()).unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// XML
// ─────────────────────────────────────────────────────────────────────────────

pub fn xml_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            _ => o.push(c),
        }
    }
    o
}

pub fn xml_attr(s: &str) -> String {
    xml_text(s).replace('"', "&quot;")
}

/// createrepo_c's `cr_is_primary`: the files worth listing in primary.xml so
/// `dnf install /usr/bin/foo` resolves without downloading filelists.
pub fn is_primary_file(path: &str) -> bool {
    path.starts_with("/etc/") || path == "/usr/lib/sendmail" || path.contains("bin/")
}

fn dep_entry(d: &Dep) -> String {
    let mut s = format!("      <rpm:entry name=\"{}\"", xml_attr(&d.name));
    if !d.flags.is_empty() {
        s.push_str(&format!(" flags=\"{}\"", d.flags));
        if let Some(e) = &d.epoch {
            s.push_str(&format!(" epoch=\"{}\"", xml_attr(e)));
        }
        if let Some(v) = &d.ver {
            s.push_str(&format!(" ver=\"{}\"", xml_attr(v)));
        }
        if let Some(r) = &d.rel {
            s.push_str(&format!(" rel=\"{}\"", xml_attr(r)));
        }
    }
    if d.pre {
        s.push_str(" pre=\"1\"");
    }
    s.push_str("/>\n");
    s
}

fn dep_section(out: &mut String, tag: &str, deps: &[Dep]) {
    if deps.is_empty() {
        return;
    }
    out.push_str(&format!("    <rpm:{tag}>\n"));
    for d in deps {
        out.push_str(&dep_entry(d));
    }
    out.push_str(&format!("    </rpm:{tag}>\n"));
}

/// The `<package>` element for primary.xml. `href` is the location relative
/// to the repo root; `file_time` is the "time file" attribute (informational).
pub fn primary_fragment(p: &RpmInfo, href: &str, file_time: u64) -> String {
    let mut o = String::with_capacity(2048);
    o.push_str("<package type=\"rpm\">\n");
    o.push_str(&format!("  <name>{}</name>\n", xml_text(&p.name)));
    o.push_str(&format!("  <arch>{}</arch>\n", xml_text(if p.is_source { "src" } else { &p.arch })));
    o.push_str(&format!("  <version epoch=\"{}\" ver=\"{}\" rel=\"{}\"/>\n", p.epoch, xml_attr(&p.version), xml_attr(&p.release)));
    o.push_str(&format!("  <checksum type=\"sha256\" pkgid=\"YES\">{}</checksum>\n", p.sha256));
    o.push_str(&format!("  <summary>{}</summary>\n", xml_text(&p.summary)));
    o.push_str(&format!("  <description>{}</description>\n", xml_text(&p.description)));
    o.push_str(&format!("  <packager>{}</packager>\n", xml_text(&p.packager)));
    o.push_str(&format!("  <url>{}</url>\n", xml_text(&p.url)));
    o.push_str(&format!("  <time file=\"{}\" build=\"{}\"/>\n", file_time, p.build_time));
    o.push_str(&format!("  <size package=\"{}\" installed=\"{}\" archive=\"{}\"/>\n", p.size_package, p.size_installed, p.size_archive));
    o.push_str(&format!("  <location href=\"{}\"/>\n", xml_attr(href)));
    o.push_str("  <format>\n");
    o.push_str(&format!("    <rpm:license>{}</rpm:license>\n", xml_text(&p.license)));
    o.push_str(&format!("    <rpm:vendor>{}</rpm:vendor>\n", xml_text(&p.vendor)));
    o.push_str(&format!("    <rpm:group>{}</rpm:group>\n", xml_text(&p.group)));
    o.push_str(&format!("    <rpm:buildhost>{}</rpm:buildhost>\n", xml_text(&p.buildhost)));
    o.push_str(&format!("    <rpm:sourcerpm>{}</rpm:sourcerpm>\n", xml_text(&p.sourcerpm)));
    o.push_str(&format!("    <rpm:header-range start=\"{}\" end=\"{}\"/>\n", p.header_start, p.header_end));
    dep_section(&mut o, "provides", &p.provides);
    dep_section(&mut o, "requires", &p.requires);
    dep_section(&mut o, "conflicts", &p.conflicts);
    dep_section(&mut o, "obsoletes", &p.obsoletes);
    dep_section(&mut o, "suggests", &p.suggests);
    dep_section(&mut o, "enhances", &p.enhances);
    dep_section(&mut o, "recommends", &p.recommends);
    dep_section(&mut o, "supplements", &p.supplements);
    for f in &p.files {
        if is_primary_file(&f.path) {
            o.push_str(&file_elem(f, "    "));
        }
    }
    o.push_str("  </format>\n</package>\n");
    o
}

fn file_elem(f: &FileEntry, indent: &str) -> String {
    if f.kind.is_empty() {
        format!("{indent}<file>{}</file>\n", xml_text(&f.path))
    } else {
        format!("{indent}<file type=\"{}\">{}</file>\n", f.kind, xml_text(&f.path))
    }
}

/// filelists.xml is where `<file>` entries in `<format>` would be silly: it
/// lists every path so `dnf provides /usr/share/x` works.
pub fn filelists_fragment(p: &RpmInfo) -> String {
    let mut o = String::with_capacity(512);
    o.push_str(&format!("<package pkgid=\"{}\" name=\"{}\" arch=\"{}\">\n", p.sha256, xml_attr(&p.name), xml_attr(if p.is_source { "src" } else { &p.arch })));
    o.push_str(&format!("  <version epoch=\"{}\" ver=\"{}\" rel=\"{}\"/>\n", p.epoch, xml_attr(&p.version), xml_attr(&p.release)));
    for f in &p.files {
        o.push_str(&file_elem(f, "  "));
    }
    o.push_str("</package>\n");
    o
}

pub fn other_fragment(p: &RpmInfo) -> String {
    let mut o = String::with_capacity(512);
    o.push_str(&format!("<package pkgid=\"{}\" name=\"{}\" arch=\"{}\">\n", p.sha256, xml_attr(&p.name), xml_attr(if p.is_source { "src" } else { &p.arch })));
    o.push_str(&format!("  <version epoch=\"{}\" ver=\"{}\" rel=\"{}\"/>\n", p.epoch, xml_attr(&p.version), xml_attr(&p.release)));
    for c in &p.changelog {
        o.push_str(&format!("  <changelog author=\"{}\" date=\"{}\">{}</changelog>\n", xml_attr(&c.author), c.date, xml_text(&c.text)));
    }
    o.push_str("</package>\n");
    o
}

pub fn primary_head(n: usize) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<metadata xmlns=\"http://linux.duke.edu/metadata/common\" xmlns:rpm=\"http://linux.duke.edu/metadata/rpm\" packages=\"{n}\">\n")
}
pub fn filelists_head(n: usize) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<filelists xmlns=\"http://linux.duke.edu/metadata/filelists\" packages=\"{n}\">\n")
}
pub fn other_head(n: usize) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<otherdata xmlns=\"http://linux.duke.edu/metadata/other\" packages=\"{n}\">\n")
}
pub const PRIMARY_TAIL: &str = "</metadata>";
pub const FILELISTS_TAIL: &str = "</filelists>";
pub const OTHER_TAIL: &str = "</otherdata>";

/// One `<data>` block's inputs for repomd.xml.
pub struct RepomdEntry {
    pub kind: &'static str,
    /// e.g. "repodata/primary.xml.gz"
    pub href: String,
    pub checksum: String,
    pub open_checksum: String,
    pub size: u64,
    pub open_size: u64,
    pub timestamp: u64,
}

/// repomd.xml: the one file dnf fetches with `metadata_expire` semantics;
/// everything else is fetched by the checksum-named href it lists, which is
/// why the metadata files are immutable and cacheable forever.
pub fn render_repomd(entries: &[RepomdEntry], revision: u64) -> String {
    let mut o = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<repomd xmlns=\"http://linux.duke.edu/metadata/repo\" xmlns:rpm=\"http://linux.duke.edu/metadata/rpm\">\n");
    o.push_str(&format!("  <revision>{revision}</revision>\n"));
    for e in entries {
        o.push_str(&format!("  <data type=\"{}\">\n", e.kind));
        o.push_str(&format!("    <checksum type=\"sha256\">{}</checksum>\n", e.checksum));
        o.push_str(&format!("    <open-checksum type=\"sha256\">{}</open-checksum>\n", e.open_checksum));
        o.push_str(&format!("    <location href=\"{}\"/>\n", xml_attr(&e.href)));
        o.push_str(&format!("    <timestamp>{}</timestamp>\n", e.timestamp));
        o.push_str(&format!("    <size>{}</size>\n", e.size));
        o.push_str(&format!("    <open-size>{}</open-size>\n", e.open_size));
        o.push_str("  </data>\n");
    }
    o.push_str("</repomd>\n");
    o
}

/// `<name>-<version>-<release>.<arch>` → the Packages/ sub-directory Fedora
/// mirrors use: first letter of the name, lower-cased. Keeps directories
/// small and matches what people expect to see when they browse.
pub fn packages_subdir(name: &str) -> String {
    name.chars().next().map(|c| c.to_ascii_lowercase().to_string()).unwrap_or_else(|| "_".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdata(f: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(f)
    }

    /// Extract one `<package …>…</package>` block by its NEVRA-ish identity
    /// (name + arch + version) from a reference file.
    fn ref_block(file: &str, name: &str, arch: &str, ver: &str) -> String {
        let s = std::fs::read_to_string(testdata(file)).unwrap();
        // "<package " with the space: "<package" alone also matches <packager>.
        for block in s.split("<package ").skip(1) {
            let block = format!("<package {}", block.split("</package>").next().unwrap());
            let has_name = block.contains(&format!("<name>{name}</name>")) || block.contains(&format!("name=\"{name}\""));
            let has_arch = block.contains(&format!("<arch>{arch}</arch>")) || block.contains(&format!("arch=\"{arch}\""));
            if has_name && has_arch && block.contains(&format!("ver=\"{ver}\"")) {
                return block + "</package>\n";
            }
        }
        panic!("no block for {name} {arch} {ver} in {file}");
    }

    /// The reference's `time file=` is the mtime on the machine that ran
    /// createrepo_c; substitute ours before comparing.
    fn norm_time(s: &str) -> String {
        let re_start = s.find("<time file=\"").expect("time elem");
        let q1 = re_start + "<time file=\"".len();
        let q2 = s[q1..].find('"').unwrap() + q1;
        format!("{}0{}", &s[..q1], &s[q2..])
    }

    fn check(rpm: &str, name: &str, arch: &str, ver: &str, refs: (&str, &str, &str)) -> RpmInfo {
        let p = read_rpm(&testdata(rpm)).unwrap();
        let got = norm_time(&primary_fragment(&p, rpm, 0));
        let want = norm_time(&ref_block(refs.0, name, arch, ver));
        assert_eq!(got, want, "primary fragment for {rpm}");
        assert_eq!(filelists_fragment(&p), ref_block(refs.1, name, arch, ver), "filelists for {rpm}");
        assert_eq!(other_fragment(&p), ref_block(refs.2, name, arch, ver), "other for {rpm}");
        p
    }

    const HELLO_REFS: (&str, &str, &str) = ("ref-primary.xml", "ref-filelists.xml", "ref-other.xml");
    const EDGE_REFS: (&str, &str, &str) = ("ref-edge-primary.xml", "ref-edge-filelists.xml", "ref-edge-other.xml");

    #[test]
    fn hello_1_0_i386_matches_createrepo_c() {
        // Pins: pre="1" from PREREQ, /bin/sh ×4 → one entry, unversioned libc
        // dropped in favour of GLIBC_2.0, appended last; primary-file filter.
        let p = check("hello-1.0-1.i386.rpm", "hello", "i386", "1.0", HELLO_REFS);
        assert_eq!(p.requires.len(), 3);
        assert_eq!(p.requires.last().unwrap().name, "libc.so.6(GLIBC_2.0)");
        assert_eq!(p.header_start, 280);
        assert_eq!(p.header_end, 2816);
    }

    #[test]
    fn hello_2_0_x86_64_matches_createrepo_c() {
        let p = check("hello-2.0-1.x86_64.rpm", "hello", "x86_64", "2.0", HELLO_REFS);
        assert_eq!(p.size_archive, 8060);
        assert_eq!(p.size_installed, 7243);
        assert_eq!(p.changelog.len(), 2);
        assert!(p.changelog[0].date < p.changelog[1].date, "oldest first");
    }

    #[test]
    fn foo_noarch_matches_createrepo_c() {
        // Pins: a package with NO requires (section omitted), no files,
        // empty packager/url/vendor still emitted as empty elements.
        let p = check("foo-1.0-1.noarch.rpm", "foo", "noarch", "1.0", HELLO_REFS);
        assert!(p.requires.is_empty());
        assert!(p.files.is_empty());
        assert_eq!(p.obsoletes.len(), 1);
    }

    #[test]
    fn edge_case_package_matches_createrepo_c() {
        // Pins: epoch 3; Requires(post) vs Requires(postun) on one name
        // survive as two entries (pre is identity); self-provided
        // config(edge) and self-shipped /usr/bin/edge-tool dropped; rich
        // deps pass through; all four weak-dep sections in createrepo_c's
        // order; ghost/dir/symlink typing; changelog escaping and order.
        let p = check("edge-1.2.3-4.fc42.noarch.rpm", "edge", "noarch", "1.2.3", EDGE_REFS);
        assert_eq!(p.epoch, 3);
        assert_eq!(p.nevra(), "edge-3:1.2.3-4.fc42.noarch");
        assert_eq!(p.filename(), "edge-1.2.3-4.fc42.noarch.rpm");
        let sh: Vec<bool> = p.requires.iter().filter(|d| d.name == "/bin/sh").map(|d| d.pre).collect();
        assert_eq!(sh, vec![true, false]);
        assert!(!p.requires.iter().any(|d| d.name == "config(edge)" || d.name == "/usr/bin/edge-tool"));
        assert!(p.requires.iter().any(|d| d.name == "(foo or bar)"));
        assert_eq!(p.files.iter().find(|f| f.path.ends_with("ghost.db")).unwrap().kind, "ghost");
        assert_eq!(p.files.iter().find(|f| f.path == "/usr/share/edge").unwrap().kind, "dir");
        assert_eq!(p.files.iter().find(|f| f.path.ends_with("link.txt")).unwrap().kind, "");
        assert_eq!(p.changelog.len(), 3);
    }

    #[test]
    fn libc_rule_picks_highest_and_treats_unversioned_as_lowest() {
        assert_eq!(glibc_version_key("libc.so.6"), Vec::<u64>::new());
        assert_eq!(glibc_version_key("libc.so.6()(64bit)"), Vec::<u64>::new());
        assert_eq!(glibc_version_key("libc.so.6(GLIBC_2.2.5)(64bit)"), vec![2, 2, 5]);
        assert!(glibc_version_key("libc.so.6(GLIBC_2.34)") > glibc_version_key("libc.so.6(GLIBC_2.4)"));
        let mk = |n: &str| Dep { name: n.into(), flags: String::new(), epoch: None, ver: None, rel: None, pre: false };
        let out = filter_requires(vec![mk("libc.so.6(GLIBC_2.34)"), mk("zlib"), mk("libc.so.6"), mk("libc.so.6(GLIBC_2.4)")], &HashSet::new(), &HashSet::new());
        assert_eq!(out.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["zlib", "libc.so.6(GLIBC_2.34)"]);
    }

    #[test]
    fn primary_file_rule() {
        assert!(is_primary_file("/etc/foo.conf"));
        assert!(is_primary_file("/usr/bin/x"));
        assert!(is_primary_file("/usr/sbin/x"));
        assert!(is_primary_file("/usr/lib/sendmail"));
        assert!(!is_primary_file("/usr/share/doc/x"));
        assert!(!is_primary_file("/usr/lib64/libx.so"));
    }

    #[test]
    fn repomd_lists_every_entry_and_not_an_rpm_is_refused() {
        let e = RepomdEntry { kind: "primary", href: "repodata/primary.xml.gz".into(), checksum: "a".repeat(64), open_checksum: "b".repeat(64), size: 10, open_size: 20, timestamp: 5 };
        let s = render_repomd(&[e], 7);
        assert!(s.contains("<revision>7</revision>"));
        assert!(s.contains("<data type=\"primary\">"));
        assert!(s.contains("<location href=\"repodata/primary.xml.gz\"/>"));
        assert!(s.contains("<open-size>20</open-size>"));
        let d = crate::tmpdir("rpm");
        let p = d.join("x.rpm");
        std::fs::write(&p, vec![0u8; 200]).unwrap();
        assert!(read_rpm(&p).is_err());
        std::fs::write(&p, b"short").unwrap();
        assert!(read_rpm(&p).is_err());
        std::fs::remove_dir_all(d).unwrap();
        assert_eq!(packages_subdir("Zsh"), "z");
    }
}
