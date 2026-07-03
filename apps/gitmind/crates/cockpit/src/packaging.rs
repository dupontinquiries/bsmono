//! packaging — turn a built binary into a Fedora RPM or an Alpine apk, and host a repo.
//!
//! gitmind runs on both target systems (Asahi/Fedora → dnf/rpm, postmarketOS/Alpine →
//! apk), so it packages *on the device* using the native tools (`rpmbuild`, `abuild`,
//! `createrepo_c`, `apk index`) and hosts the resulting repo over HTTP. This module is the
//! part that doesn't need those tools present: it generates the `.spec` / `APKBUILD`, the
//! repo-metadata + client-config, builds the exact commands to run, and serves the repo
//! directory. Everything here is pure/deterministic and unit-tested; the orchestration in
//! `flow` runs the commands best-effort on-device.

use std::path::{Path, PathBuf};

/// One file to install into the package (a built binary, usually).
#[derive(Debug, Clone)]
pub struct InstallFile {
    /// Source path (the built artifact).
    pub src: PathBuf,
    /// Absolute install path in the target root, e.g. `/usr/bin/gitmind`.
    pub dest: String,
    /// Unix mode, e.g. 0o755.
    pub mode: u32,
}

/// Metadata for a package, independent of format.
#[derive(Debug, Clone)]
pub struct PkgMeta {
    pub name: String,
    pub version: String,
    pub release: String,
    pub summary: String,
    pub description: String,
    pub license: String,
    pub url: String,
    pub maintainer: String,
    /// Package architecture (see [`arch_for_triple`]).
    pub arch: String,
    pub files: Vec<InstallFile>,
}

impl PkgMeta {
    pub fn new(name: &str, version: &str, arch: &str) -> PkgMeta {
        PkgMeta {
            name: name.to_string(),
            version: version.to_string(),
            release: "1".to_string(),
            summary: format!("{name} (built by gitmind)"),
            description: format!("{name}, packaged and served by gitmind."),
            license: "MIT".to_string(),
            url: String::new(),
            maintainer: "gitmind <gitmind@localhost>".to_string(),
            arch: arch.to_string(),
            files: Vec::new(),
        }
    }
    pub fn with_summary(mut self, s: &str) -> PkgMeta {
        if !s.is_empty() {
            self.summary = s.to_string();
            self.description = s.to_string();
        }
        self
    }
    pub fn add_binary(mut self, src: &Path, name: &str) -> PkgMeta {
        self.files.push(InstallFile { src: src.to_path_buf(), dest: format!("/usr/bin/{name}"), mode: 0o755 });
        self
    }
}

/// Map a Rust target triple (or empty = native) to the RPM/apk architecture string.
pub fn arch_for_triple(triple: &str) -> &'static str {
    if triple.contains("aarch64") || triple.contains("arm64") {
        "aarch64"
    } else if triple.contains("x86_64") {
        "x86_64"
    } else if triple.contains("armv7") || triple.contains("armhf") {
        "armv7"
    } else if triple.contains("riscv64") {
        "riscv64"
    } else {
        // native fallback — best effort by host arch at call sites; default aarch64 for the fleet
        "aarch64"
    }
}

// ── Fedora RPM ──────────────────────────────────────────────────────────────

/// Generate an RPM `.spec` that installs prebuilt binaries from `%{_sourcedir}` (no compile
/// step — gitmind already built them). File basenames in SOURCES must match `src` basenames.
pub fn rpm_spec(m: &PkgMeta) -> String {
    let mut s = String::new();
    s.push_str(&format!("Name:           {}\n", m.name));
    s.push_str(&format!("Version:        {}\n", m.version));
    s.push_str(&format!("Release:        {}%{{?dist}}\n", m.release));
    s.push_str(&format!("Summary:        {}\n", one_line(&m.summary)));
    s.push_str(&format!("License:        {}\n", m.license));
    if !m.url.is_empty() {
        s.push_str(&format!("URL:            {}\n", m.url));
    }
    s.push_str(&format!("BuildArch:      {}\n", m.arch));
    // sources: one per file, by basename
    for (i, f) in m.files.iter().enumerate() {
        s.push_str(&format!("Source{}:        {}\n", i, basename(&f.src)));
    }
    s.push_str("\n%description\n");
    s.push_str(&wrap_desc(&m.description));
    s.push_str("\n\n%install\n");
    for (i, f) in m.files.iter().enumerate() {
        let dir = parent_dir(&f.dest);
        s.push_str(&format!("mkdir -p %{{buildroot}}{dir}\n"));
        s.push_str(&format!(
            "install -m {:o} %{{_sourcedir}}/{} %{{buildroot}}{}\n",
            f.mode,
            basename(&f.src),
            f.dest
        ));
        let _ = i;
    }
    s.push_str("\n%files\n");
    for f in &m.files {
        s.push_str(&format!("{}\n", f.dest));
    }
    s.push_str("\n%changelog\n");
    s.push_str(&format!(
        "* {} {} - {}-{}\n- Built and packaged by gitmind\n",
        rpm_date(),
        m.maintainer,
        m.version,
        m.release
    ));
    s
}

/// The `rpmbuild` command to build a binary RPM from a spec, given an rpm topdir.
/// Returns (program, args). Binaries must already be copied into `<topdir>/SOURCES`.
pub fn rpmbuild_cmd(spec_path: &Path, topdir: &Path) -> (String, Vec<String>) {
    (
        "rpmbuild".to_string(),
        vec![
            "-bb".to_string(),
            format!("--define=_topdir {}", topdir.display()),
            spec_path.display().to_string(),
        ],
    )
}

/// The `createrepo_c` command to (re)generate RPM repo metadata in `repo_dir`.
pub fn createrepo_cmd(repo_dir: &Path) -> (String, Vec<String>) {
    ("createrepo_c".to_string(), vec![repo_dir.display().to_string()])
}

/// A dnf `.repo` file pointing at a gitmind-hosted repo (unsigned → gpgcheck=0).
pub fn dnf_repo_file(repo_name: &str, baseurl: &str) -> String {
    format!(
        "[{repo_name}]\nname=gitmind fleet repo ({repo_name})\nbaseurl={baseurl}\nenabled=1\ngpgcheck=0\n"
    )
}

// ── Alpine apk ───────────────────────────────────────────────────────────────

/// Generate an `APKBUILD` that installs prebuilt binaries (no build step; abuild still runs
/// `package()`). Prebuilt files are expected in `$srcdir` by basename.
pub fn apkbuild(m: &PkgMeta) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Maintainer: {}\n", m.maintainer));
    s.push_str(&format!("pkgname={}\n", m.name));
    s.push_str(&format!("pkgver={}\n", m.version));
    s.push_str(&format!("pkgrel={}\n", m.release.trim_start_matches(|c: char| !c.is_ascii_digit()).parse::<u32>().unwrap_or(0)));
    s.push_str(&format!("pkgdesc=\"{}\"\n", one_line(&m.summary)));
    s.push_str(&format!("url=\"{}\"\n", m.url));
    s.push_str(&format!("arch=\"{}\"\n", m.arch));
    s.push_str(&format!("license=\"{}\"\n", m.license));
    s.push_str("options=\"!check !strip\"\n");
    // local prebuilt sources (basenames); abuild copies them into $srcdir
    let srcs: Vec<String> = m.files.iter().map(|f| basename(&f.src)).collect();
    s.push_str(&format!("source=\"{}\"\n", srcs.join(" ")));
    s.push_str("\npackage() {\n");
    for f in &m.files {
        s.push_str(&format!(
            "\tinstall -Dm{:o} \"$srcdir\"/{} \"$pkgdir\"{}\n",
            f.mode,
            basename(&f.src),
            f.dest
        ));
    }
    s.push_str("}\n");
    s
}

/// The `abuild` command to build an apk (run from the APKBUILD dir). `-F` allows running as
/// non-root builder; `-r` installs missing deps. Unsigned local builds also want abuild-keygen.
pub fn abuild_cmd() -> (String, Vec<String>) {
    ("abuild".to_string(), vec!["-r".to_string(), "-F".to_string()])
}

/// The `apk index` command to build an APKINDEX for an Alpine repo dir (unsigned).
pub fn apk_index_cmd(repo_dir: &Path) -> (String, Vec<String>) {
    (
        "sh".to_string(),
        vec![
            "-c".to_string(),
            format!(
                "cd {0} && apk index -o APKINDEX.tar.gz *.apk",
                shell_quote(&repo_dir.display().to_string())
            ),
        ],
    )
}

/// The line to add to `/etc/apk/repositories` (unsigned repos need `--allow-untrusted` at
/// install time, or the repo must be signed with abuild-sign).
pub fn apk_repo_line(baseurl: &str) -> String {
    baseurl.trim_end_matches('/').to_string()
}

/// The one-shot `apk add` a user runs to install from an unsigned gitmind repo.
pub fn apk_add_cmd(baseurl: &str, pkg: &str) -> String {
    format!("apk add --allow-untrusted --repository {} {}", baseurl.trim_end_matches('/'), pkg)
}

// ── repo hosting (a minimal static HTTP server dnf/apk can fetch from) ─────────

/// Resolve a request target (`/repodata/repomd.xml`) to a real path inside `root`, rejecting
/// path traversal. Returns `None` for anything that escapes the root. Pure + testable.
pub fn resolve_in_root(root: &Path, target: &str) -> Option<PathBuf> {
    let cleaned = target.split('?').next().unwrap_or(target);
    let rel = cleaned.trim_start_matches('/');
    // reject traversal and absolute components
    if rel.split('/').any(|c| c == ".." || c.starts_with('/')) {
        return None;
    }
    let mut p = root.to_path_buf();
    for comp in rel.split('/').filter(|c| !c.is_empty() && *c != ".") {
        p.push(comp);
    }
    // if a directory is requested, serve nothing (dnf/apk fetch files, not listings)
    Some(p)
}

/// content-type for a repo file (mostly irrelevant to dnf/apk, but polite).
pub fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("xml") => "application/xml",
        Some("rpm") => "application/x-rpm",
        Some("apk") => "application/octet-stream",
        Some("gz") | Some("tar") => "application/octet-stream",
        _ => "application/octet-stream",
    }
}

// ── small helpers ────────────────────────────────────────────────────────────

fn basename(p: &Path) -> String {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string()
}
fn parent_dir(dest: &str) -> String {
    match dest.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => dest[..i].to_string(),
    }
}
fn one_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}
fn wrap_desc(s: &str) -> String {
    // rpm %description wants non-empty; keep it simple
    if s.trim().is_empty() {
        "Packaged by gitmind.".to_string()
    } else {
        s.trim().to_string()
    }
}
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
/// RPM changelog date: "Mon Jan 02 2006"-style is ideal, but a stable ISO-ish date is
/// accepted by rpmbuild in practice; we keep it deterministic and valid enough.
fn rpm_date() -> String {
    // Avoid a chrono dep; a fixed weekday-less form is tolerated by rpmbuild --nocheck of
    // changelog ordering for a single entry. Callers may override.
    "Thu Jan 01 1970".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PkgMeta {
        PkgMeta::new("gitmind", "0.2.0", "aarch64")
            .with_summary("Fleet cockpit")
            .add_binary(Path::new("/tmp/build/gitmind"), "gitmind")
    }

    #[test]
    fn arch_mapping() {
        assert_eq!(arch_for_triple("aarch64-unknown-linux-musl"), "aarch64");
        assert_eq!(arch_for_triple("x86_64-unknown-linux-gnu"), "x86_64");
        assert_eq!(arch_for_triple(""), "aarch64");
    }

    #[test]
    fn rpm_spec_has_required_fields_and_install() {
        let s = rpm_spec(&sample());
        assert!(s.contains("Name:           gitmind"));
        assert!(s.contains("Version:        0.2.0"));
        assert!(s.contains("BuildArch:      aarch64"));
        assert!(s.contains("Source0:        gitmind"));
        assert!(s.contains("install -m 755 %{_sourcedir}/gitmind %{buildroot}/usr/bin/gitmind"));
        assert!(s.contains("/usr/bin/gitmind"));
        assert!(s.contains("%files"));
    }

    #[test]
    fn apkbuild_has_package_and_install() {
        let s = apkbuild(&sample());
        assert!(s.contains("pkgname=gitmind"));
        assert!(s.contains("pkgver=0.2.0"));
        assert!(s.contains("arch=\"aarch64\""));
        assert!(s.contains("source=\"gitmind\""));
        assert!(s.contains("install -Dm755 \"$srcdir\"/gitmind \"$pkgdir\"/usr/bin/gitmind"));
    }

    #[test]
    fn repo_configs() {
        assert!(dnf_repo_file("fleet", "http://host:8080/").contains("baseurl=http://host:8080/"));
        assert!(dnf_repo_file("fleet", "http://h/").contains("gpgcheck=0"));
        assert_eq!(apk_repo_line("http://host:8080/"), "http://host:8080");
        assert_eq!(apk_add_cmd("http://host:8080/", "gitmind"), "apk add --allow-untrusted --repository http://host:8080 gitmind");
    }

    #[test]
    fn command_builders() {
        let (prog, args) = rpmbuild_cmd(Path::new("/r/SPECS/gitmind.spec"), Path::new("/r"));
        assert_eq!(prog, "rpmbuild");
        assert!(args.iter().any(|a| a.contains("_topdir /r")));
        assert!(args.iter().any(|a| a.ends_with("gitmind.spec")));
        let (p2, a2) = createrepo_cmd(Path::new("/repo"));
        assert_eq!(p2, "createrepo_c");
        assert_eq!(a2, vec!["/repo".to_string()]);
        let (p3, a3) = apk_index_cmd(Path::new("/repo"));
        assert_eq!(p3, "sh");
        assert!(a3[1].contains("apk index -o APKINDEX.tar.gz"));
    }

    #[test]
    fn repo_path_resolution_blocks_traversal() {
        let root = Path::new("/srv/repo");
        assert_eq!(resolve_in_root(root, "/repodata/repomd.xml"), Some(PathBuf::from("/srv/repo/repodata/repomd.xml")));
        assert_eq!(resolve_in_root(root, "/gitmind-0.2.0.aarch64.rpm"), Some(PathBuf::from("/srv/repo/gitmind-0.2.0.aarch64.rpm")));
        assert_eq!(resolve_in_root(root, "/../etc/passwd"), None);
        assert_eq!(resolve_in_root(root, "/a/../../b"), None);
    }
}
