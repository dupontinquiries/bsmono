//! alpine_root: packages on a reMarkable, the way a laptop has them.
//!
//! The tablet's OS is a read-only, vendor-built system with no package
//! manager gitmind can drive and no audio stack (BlueZ is there; nothing
//! routes audio to a headset). So gitmind keeps an Alpine Linux userland
//! under `/home` (persistent across reboots and OS updates; the system is
//! never unlocked) and manages it with Alpine's own `apk.static`:
//!
//! * `bootstrap`: using only the tablet's `curl` and `python3`, fetch the
//!   repository index, get `apk-tools-static` and `alpine-keys`, extract
//!   `apk.static` and the signing keys, and `--initdb` a root. From then on
//!   every install is signature-checked, exactly as on Alpine.
//! * `add` / `del` / `list`: `apk.static --root <root> ...`.
//! * run: `alpine-run <root> <program>` (shared/eink-kit/alpine-run): a
//!   private mount namespace with /dev /run /tmp /home bound in, chroot,
//!   exec. Audio then works like a laptop: `bluez-alsa` from Alpine talks to
//!   the tablet's own BlueZ over D-Bus and apps get an ALSA device.
//!
//! Why python3 for extraction: an `.apk` is three gzipped tar archives
//! concatenated (signature, control, data). `tar` stops after the first;
//! Python's `tarfile` with `ignore_zeros` reads all three, and the tablet
//! ships python3.12.
//!
//! The mirror is Alpine's CDN by default, or depot on the LAN (its apk
//! projection) for offline fleets. Every command here is shell text,
//! tested under sh against a fake mirror (curl file://).

use crate::deploy::shell_quote;

/// Where the Alpine root lives on the tablet (persistent tree).
pub const ROOT: &str = "/home/root/.local/share/gitmind/alpine";
/// The runner binary gitmind installs next to its apps.
pub const RUNNER: &str = "/home/root/.local/share/gitmind/bin/alpine-run";
pub const DEFAULT_MIRROR: &str = "https://dl-cdn.alpinelinux.org/alpine";
/// Alpine branch. Must match the build container's Alpine for apps linked
/// dynamically against the root's libraries (rust:1-alpine tracks the
/// latest release too). UNVERIFIED pairing: check /etc/alpine-release in
/// both when a dynamically linked app fails to start.
pub const DEFAULT_BRANCH: &str = "latest-stable";
/// What a bare root needs to be usable (a shell, the base layout, apk).
pub const BASE_PACKAGES: &[&str] = &["alpine-baselayout", "busybox", "apk-tools", "ca-certificates"];

/// Where packages come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mirror {
    /// e.g. https://dl-cdn.alpinelinux.org/alpine, or a depot URL.
    pub base: String,
    pub branch: String,
}

impl Default for Mirror {
    fn default() -> Self {
        Mirror { base: DEFAULT_MIRROR.into(), branch: DEFAULT_BRANCH.into() }
    }
}

impl Mirror {
    pub fn repo(&self, which: &str) -> String {
        format!("{}/{}/{which}", self.base.trim_end_matches('/'), self.branch)
    }
}

/// Where depot's API would be for a mirror URL of depot's shape
/// (`<depot>/alpine`). Alpine's own mirrors ALSO end in `/alpine`, so they
/// are excluded by host; anything else is confirmed by the listing itself
/// (`check_mirror` treats a non-listing answer as "not a depot").
pub fn depot_api(m: &Mirror) -> Option<String> {
    let base = m.base.trim_end_matches('/');
    let host = base.split("://").nth(1).unwrap_or(base).split('/').next().unwrap_or("");
    if host.ends_with("alpinelinux.org") {
        return None;
    }
    base.strip_suffix("/alpine").map(|b| format!("{b}/api/alpine"))
}

/// What a depot can and cannot satisfy for an install.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DepotCheck {
    /// Packages in the dependency closure.
    pub needed: usize,
    /// (repo key, file name) named by the closure but not ingested.
    pub missing: Vec<(String, String)>,
    /// Dependencies no package in the depot's indexes provides.
    pub unresolved: Vec<String>,
    /// Repos the tablet needs that the depot does not have at all.
    pub absent_repos: Vec<String>,
}

impl DepotCheck {
    pub fn ok(&self) -> bool {
        self.missing.is_empty() && self.unresolved.is_empty() && self.absent_repos.is_empty()
    }

    /// Commands for the ONLINE side to fetch exactly what is missing from
    /// Alpine's CDN, laid out for `depot ingest alpine`.
    pub fn fetch_script(&self) -> String {
        let mut s = String::from("# on a machine with internet access:\n");
        for (key, file) in &self.missing {
            s += &format!("mkdir -p {key} && curl -fsSLo {key}/{file} {DEFAULT_MIRROR}/{key}/{file}\n");
        }
        for key in &self.absent_repos {
            s += &format!("mkdir -p {key} && curl -fsSLo {key}/APKINDEX.tar.gz {DEFAULT_MIRROR}/{key}/APKINDEX.tar.gz\n");
        }
        s += "depot ingest alpine .   # then move it to the depot, or pack it: depot pack --alpine\n";
        s
    }
}

/// Pure: can the depot described by `listing`, whose repos' indexes are
/// `indexes` ((key, packages)), satisfy installing `wanted`?
pub fn check_depot(listing: &depot_core::api::AlpineListing, indexes: &[(String, Vec<depot_core::alpine::IndexPkg>)], wanted: &[&str], needed_repos: &[String]) -> DepotCheck {
    let mut all = Vec::new();
    let mut repo_of = std::collections::BTreeMap::new();
    for (key, pkgs) in indexes {
        for p in pkgs {
            repo_of.entry(p.name.clone()).or_insert_with(|| key.clone());
            all.push(p.clone());
        }
    }
    let c = depot_core::alpine::closure(&all, wanted);
    let mut missing = Vec::new();
    for p in &c.needed {
        let key = &repo_of[&p.name];
        let have = listing.repo(key).is_some_and(|r| r.packages.iter().any(|x| x.name == p.name && x.version == p.version));
        if !have {
            missing.push((key.clone(), p.filename()));
        }
    }
    DepotCheck {
        needed: c.needed.len(),
        missing,
        unresolved: c.unresolved,
        absent_repos: needed_repos.iter().filter(|k| listing.repo(k).is_none()).cloned().collect(),
    }
}

fn curl(url: &str) -> Result<Vec<u8>, String> {
    let o = std::process::Command::new("curl").args(["-fsS", "--max-time", "20", url]).output().map_err(|e| format!("curl: {e}"))?;
    if !o.status.success() {
        return Err(format!("{url}: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(o.stdout)
}

/// Ask the depot behind `m` whether it can satisfy installing `wanted` on a
/// tablet of `arch`. None when `m` is not a depot.
pub fn check_mirror(m: &Mirror, arch: &str, wanted: &[&str]) -> Option<Result<DepotCheck, String>> {
    let api = depot_api(m)?;
    // Not answering with a depot listing = not a depot (a plain mirror
    // behind some other URL): no check, apk reports as usual.
    let listing: depot_core::api::AlpineListing = serde_json::from_slice(&curl(&api).ok()?).ok()?;
    Some((|| {
        let needed_repos: Vec<String> = ["main", "community"].iter().map(|r| format!("{}/{r}/{arch}", m.branch)).collect();
        let mut indexes = Vec::new();
        for key in needed_repos.iter().filter(|k| listing.repo(k).is_some()) {
            let bytes = curl(&format!("{}/{key}/APKINDEX.tar.gz", m.base.trim_end_matches('/')))?;
            indexes.push((key.clone(), depot_core::alpine::parse_index(&bytes).map_err(|e| format!("{key}: {e}"))?));
        }
        // community is optional: only main must exist
        let must: Vec<String> = needed_repos.into_iter().take(1).collect();
        Ok(check_depot(&listing, &indexes, wanted, &must))
    })())
}

fn valid_pkg(p: &str) -> bool {
    // plain names plus apk provider names (so:libasound.so.2, cmd:aplay)
    !p.is_empty() && p.len() <= 128 && !p.starts_with(':') && p.chars().all(|c| c.is_ascii_alphanumeric() || "+-._:".contains(c))
}

fn check_pkgs(pkgs: &[&str]) -> Result<String, String> {
    if pkgs.is_empty() {
        return Err("no packages named".into());
    }
    for p in pkgs {
        if !valid_pkg(p) {
            return Err(format!("refusing package name {p:?}"));
        }
    }
    Ok(pkgs.join(" "))
}

const EXTRACT_PY: &str = "import sys,tarfile\nt=tarfile.open(sys.argv[1],'r:gz',ignore_zeros=True)\nm=[x for x in t.getmembers() if not x.name.startswith('.')]\nt.extractall(sys.argv[2],members=m,filter='data')\n";

/// Bootstrap the Alpine root (idempotent: an existing root is kept).
pub fn bootstrap_cmd(m: &Mirror) -> String {
    format!(
        r#"set -e
R={root}; M={main}; C={community}
A=$(uname -m)
if [ -x "$R/.gitmind/apk.static" ] && [ -f "$R/lib/apk/db/installed" ]; then echo "alpine root already present"; exit 0; fi
mkdir -p "$R/.gitmind" "$R/etc/apk"
cd "$R/.gitmind"
curl -fsSL "$M/$A/APKINDEX.tar.gz" -o APKINDEX.tar.gz
# The index, like every .apk, is a signature segment then the content: read
# past the first segment (plain tarfile.open stops at it; real Alpine indexes
# ALWAYS have one, which an unsigned test index hid until depot served one).
python3 -c 'import tarfile;t=tarfile.open("APKINDEX.tar.gz","r:gz",ignore_zeros=True);t.extract("APKINDEX",".",filter="data")'
ver() {{ python3 - "$1" <<'PY'
import sys
want=sys.argv[1]
for block in open("APKINDEX").read().split("\n\n"):
    f=dict(l.split(":",1) for l in block.splitlines() if len(l)>2 and l[1]==":")
    if f.get("P")==want:
        print(f["V"]); break
PY
}}
cat > extract.py <<'PY'
{extract}PY
for p in apk-tools-static alpine-keys; do
  v=$(ver "$p"); [ -n "$v" ] || {{ echo "$p not in the repository index" >&2; exit 1; }}
  curl -fsSL "$M/$A/$p-$v.apk" -o "$p.apk"
  echo "fetched $p-$v"
done
python3 extract.py apk-tools-static.apk stage
python3 extract.py alpine-keys.apk "$R"
cp stage/sbin/apk.static "$R/.gitmind/apk.static"
chmod 755 "$R/.gitmind/apk.static"
printf '%s\n%s\n' "$M" "$C" > "$R/etc/apk/repositories"
"$R/.gitmind/apk.static" --root "$R" --initdb --arch "$A" --repositories-file "$R/etc/apk/repositories" add {base}
echo "alpine root ready at $R"
"#,
        root = shell_quote(ROOT),
        main = shell_quote(&m.repo("main")),
        community = shell_quote(&m.repo("community")),
        extract = EXTRACT_PY,
        base = BASE_PACKAGES.join(" "),
    )
}

fn apk(args: &str) -> String {
    format!(
        "R={root}; [ -x \"$R/.gitmind/apk.static\" ] || {{ echo 'no alpine root yet: run bootstrap first' >&2; exit 1; }}; \
         \"$R/.gitmind/apk.static\" --root \"$R\" --repositories-file \"$R/etc/apk/repositories\" {args}",
        root = shell_quote(ROOT)
    )
}

pub fn add_cmd(pkgs: &[&str]) -> Result<String, String> {
    Ok(apk(&format!("add {}", check_pkgs(pkgs)?)))
}

pub fn del_cmd(pkgs: &[&str]) -> Result<String, String> {
    Ok(apk(&format!("del {}", check_pkgs(pkgs)?)))
}

pub fn list_cmd() -> String {
    apk("info")
}

pub fn update_cmd() -> String {
    apk("update && \"$R/.gitmind/apk.static\" --root \"$R\" --repositories-file \"$R/etc/apk/repositories\" upgrade")
}

/// Run a program inside the root (the runner must be installed).
pub fn run_cmd(program: &str, args: &[&str]) -> String {
    let mut s = format!("{} {} {}", shell_quote(RUNNER), shell_quote(ROOT), shell_quote(program));
    for a in args {
        s.push(' ');
        s.push_str(&shell_quote(a));
    }
    s
}

/// Probe lines for `eink_fleet::displays_probe_cmd`-style parsing.
pub fn probe_cmd() -> String {
    format!(
        "R={root}; [ -x \"$R/.gitmind/apk.static\" ] && echo alpine_root=1; \
         [ -x {runner} ] && echo alpine_runner=1; \
         [ -f \"$R/etc/alpine-release\" ] && echo \"alpine_release=$(cat \"$R/etc/alpine-release\")\"; \
         pgrep -x bluealsa >/dev/null 2>&1 && echo bluealsa_running=1; true",
        root = shell_quote(ROOT),
        runner = shell_quote(RUNNER)
    )
}

/// What `probe_cmd` found on a tablet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootState {
    pub root: bool,
    pub runner: bool,
    /// `/etc/alpine-release` inside the root, e.g. `3.22.1`.
    pub release: Option<String>,
    pub bluealsa: bool,
}

pub fn parse_probe(text: &str) -> RootState {
    let mut s = RootState::default();
    for l in text.lines() {
        match l.trim().split_once('=') {
            Some(("alpine_root", "1")) => s.root = true,
            Some(("alpine_runner", "1")) => s.runner = true,
            Some(("alpine_release", v)) if !v.trim().is_empty() => s.release = Some(v.trim().to_string()),
            Some(("bluealsa_running", "1")) => s.bluealsa = true,
            _ => {}
        }
    }
    s
}

impl RootState {
    /// Why an app cannot be deployed into the root yet, if it cannot.
    pub fn not_ready(&self) -> Option<String> {
        if !self.root {
            Some("no Alpine root on this tablet yet: `gitmind eink pkg <host> bootstrap` (Packages on the Tablets page)".into())
        } else if !self.runner {
            Some("alpine-run is not installed: build it (`gitmind eink build <host> alpine-run`) and install it (`gitmind eink pkg <host> runner <binary>`)".into())
        } else {
            None
        }
    }
}

/// The build image for a root at `release`: a dynamically linked app must be
/// built against the SAME Alpine release as the root it runs in (a newer
/// musl or library soname in the build than in the root fails at start).
/// Docker Hub's official rust image is tagged per Alpine minor release.
pub fn image_for_release(release: &str) -> Option<String> {
    let mut it = release.trim().split('.');
    let (maj, min) = (it.next()?, it.next()?);
    if maj.chars().all(|c| c.is_ascii_digit()) && min.chars().all(|c| c.is_ascii_digit()) && !maj.is_empty() && !min.is_empty() {
        Some(format!("docker.io/library/rust:1-alpine{maj}.{min}"))
    } else {
        None
    }
}

/// Start the Bluetooth audio bridge (bluez-alsa) inside the root, detached,
/// for this boot: A2DP to a headset and HFP for calls. Needs `bluez-alsa`
/// installed in the root and the tablet's BlueZ running.
pub fn bluealsa_start_cmd() -> String {
    format!(
        "pgrep -x bluealsa >/dev/null 2>&1 && {{ echo 'bluealsa already running'; exit 0; }}; \
         mkdir -p /home/root/.local/share/gitmind/log; \
         nohup {run} </dev/null >/home/root/.local/share/gitmind/log/bluealsa.log 2>&1 & \
         sleep 2; pgrep -x bluealsa >/dev/null && echo 'bluealsa running' || {{ echo 'bluealsa did not start:'; tail -5 /home/root/.local/share/gitmind/log/bluealsa.log; exit 1; }}",
        run = run_cmd("/usr/bin/bluealsa", &["-p", "a2dp-source", "-p", "hfp-ag"])
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A gzipped tar holding `files` (path, contents, mode).
    fn tgz(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!("tgz_{}_{}", std::process::id(), files.len() + files.first().map(|f| f.0.len()).unwrap_or(0)));
        let _ = std::fs::remove_dir_all(&dir);
        for (p, c, mode) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, c).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(*mode)).unwrap();
        }
        let names: Vec<&str> = files.iter().map(|f| f.0).collect();
        let out = std::process::Command::new("tar").arg("-czf").arg("-").arg("-C").arg(&dir).args(&names).output().unwrap();
        assert!(out.status.success());
        let _ = std::fs::remove_dir_all(&dir);
        out.stdout
    }

    /// An .apk the way Alpine builds one: three gzipped tars concatenated.
    fn apk_file(data: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut v = tgz(&[(".SIGN.RSA.alpine-devel@lists.alpinelinux.org.rsa.pub", b"sig", 0o644)]);
        v.extend(tgz(&[(".PKGINFO", b"pkgname = x\n", 0o644)]));
        v.extend(tgz(data));
        v
    }

    #[test]
    fn bootstrap_against_a_fake_mirror_extracts_apk_static_and_keys_and_inits_the_root() {
        // no python3/curl/tar: cannot run the real script here
        for tool in ["python3", "curl", "tar", "sh"] {
            if std::process::Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().map(|o| !o.status.success()).unwrap_or(true) {
                return;
            }
        }
        let base = std::env::temp_dir().join(format!("alpmirror_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let arch = String::from_utf8(std::process::Command::new("uname").arg("-m").output().unwrap().stdout).unwrap().trim().to_string();
        let main = base.join("latest-stable/main").join(&arch);
        std::fs::create_dir_all(&main).unwrap();
        // APKINDEX with the two packages bootstrap needs
        let index = "C:Q1x\nP:apk-tools-static\nV:2.14.9-r2\nA:x\n\nC:Q1y\nP:alpine-keys\nV:2.5-r0\nA:x\n\n";
        // Realistic: a signature segment FIRST, then the index (the unsigned
        // single-segment index this test used to build hid a real bug).
        let mut signed_index = tgz(&[(".SIGN.RSA.alpine-devel@lists.alpinelinux.org.rsa.pub", b"sig", 0o644)]);
        signed_index.extend(tgz(&[("APKINDEX", index.as_bytes(), 0o644)]));
        std::fs::write(main.join("APKINDEX.tar.gz"), signed_index).unwrap();
        // a fake apk.static that records its arguments and lays down the db
        let fake = b"#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/apk-args\"\nmkdir -p \"$2/lib/apk/db\"; touch \"$2/lib/apk/db/installed\"\n";
        std::fs::write(main.join("apk-tools-static-2.14.9-r2.apk"), apk_file(&[("sbin/apk.static", fake, 0o755)])).unwrap();
        std::fs::write(main.join("alpine-keys-2.5-r0.apk"), apk_file(&[("etc/apk/keys/alpine-devel.rsa.pub", b"KEY", 0o644)])).unwrap();

        let root = base.join("tablet-root");
        let mirror = Mirror { base: format!("file://{}", base.display()), branch: "latest-stable".into() };
        let script = bootstrap_cmd(&mirror).replace(ROOT, &root.to_string_lossy());
        let out = std::process::Command::new("sh").arg("-c").arg(&script).output().unwrap();
        let so = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "stdout: {so}\nstderr: {}", String::from_utf8_lossy(&out.stderr));
        assert!(so.contains("fetched apk-tools-static-2.14.9-r2") && so.contains("alpine root ready"), "{so}");
        // all three segments were read: the key from the DATA segment landed
        assert_eq!(std::fs::read_to_string(root.join("etc/apk/keys/alpine-devel.rsa.pub")).unwrap(), "KEY");
        assert!(!root.join(".PKGINFO").exists(), "control files are not extracted into the root");
        let args = std::fs::read_to_string(root.join(".gitmind/apk-args")).unwrap();
        assert!(args.contains("--initdb") && args.contains(&format!("--arch {arch}")) && args.contains("add alpine-baselayout busybox apk-tools"), "{args}");
        let repos = std::fs::read_to_string(root.join("etc/apk/repositories")).unwrap();
        assert!(repos.contains("latest-stable/main") && repos.contains("latest-stable/community"));
        // second run is a no-op
        let again = std::process::Command::new("sh").arg("-c").arg(&script).output().unwrap();
        assert!(String::from_utf8_lossy(&again.stdout).contains("already present"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn package_commands_validate_names_and_need_a_root() {
        assert!(add_cmd(&["bluez-alsa", "alsa-utils"]).unwrap().contains("add bluez-alsa alsa-utils"));
        assert!(add_cmd(&["x; rm -rf /"]).is_err());
        assert!(add_cmd(&["so:libasound.so.2", "cmd:aplay"]).unwrap().contains("add so:libasound.so.2 cmd:aplay"), "apk provider names");
        assert!(add_cmd(&[":x"]).is_err() && add_cmd(&["so:a;b"]).is_err());
        assert!(add_cmd(&[]).is_err());
        assert!(del_cmd(&["pipewire"]).unwrap().contains("del pipewire"));
        // without a root the command says so (runs under sh against an empty tree)
        let dir = std::env::temp_dir().join(format!("noroot_{}", std::process::id()));
        let out = std::process::Command::new("sh").arg("-c").arg(list_cmd().replace(ROOT, &dir.to_string_lossy())).output().unwrap();
        assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("bootstrap first"));
    }

    #[test]
    fn run_and_bluealsa_go_through_the_runner() {
        assert_eq!(run_cmd("/usr/bin/bluealsa", &["-p", "a2dp-source"]),
            format!("'{RUNNER}' '{ROOT}' '/usr/bin/bluealsa' '-p' 'a2dp-source'"));
        let b = bluealsa_start_cmd();
        assert!(b.contains("a2dp-source") && b.contains("hfp-ag") && b.contains(RUNNER));
        let o = std::process::Command::new("sh").arg("-n").arg("-c").arg(&b).output().unwrap();
        assert!(o.status.success());
    }

    #[test]
    fn a_depot_check_names_every_missing_dependency_and_how_to_fetch_it() {
        use depot_core::api::{AlpineListing, AlpinePkgListing, AlpineRepoListing};
        let idx = depot_core::alpine::parse_index_text(concat!(
            "P:bluez-alsa\nV:4-r0\nD:so:libasound.so.2 dbus-libs\n\n",
            "P:alsa-lib\nV:1-r0\np:so:libasound.so.2=2\n\n",
            "P:dbus-libs\nV:1.14-r0\n\n"));
        let key = "latest-stable/main/aarch64".to_string();
        let pkg = |n: &str, v: &str| AlpinePkgListing { name: n.into(), version: v.into(), size: 1 };
        let listing = AlpineListing { repos: vec![AlpineRepoListing { repo: key.clone(), url: String::new(), index_size: 1,
            packages: vec![pkg("bluez-alsa", "4-r0"), pkg("dbus-libs", "1.14-r0")] }] };
        let c = check_depot(&listing, &[(key.clone(), idx)], &["bluez-alsa"], &[key.clone()]);
        assert_eq!(c.needed, 3);
        assert_eq!(c.missing, vec![(key.clone(), "alsa-lib-1-r0.apk".to_string())], "found through the so: provide");
        assert!(!c.ok());
        let script = c.fetch_script();
        assert!(script.contains("curl -fsSLo latest-stable/main/aarch64/alsa-lib-1-r0.apk https://dl-cdn.alpinelinux.org/alpine/latest-stable/main/aarch64/alsa-lib-1-r0.apk"), "{script}");
        assert!(script.contains("depot ingest alpine ."));
        // a depot without the tablet's repo at all
        let empty = AlpineListing::default();
        assert_eq!(check_depot(&empty, &[], &["x"], &[key.clone()]).absent_repos, vec![key]);
    }

    #[test]
    fn only_depot_mirrors_have_an_api() {
        assert_eq!(depot_api(&Mirror { base: "http://depot.lan:8000/alpine/".into(), branch: "v3.22".into() }).as_deref(), Some("http://depot.lan:8000/api/alpine"));
        assert!(depot_api(&Mirror::default()).is_none(), "Alpine's CDN is not asked");
    }

    #[test]
    fn the_root_state_decides_readiness_and_the_build_image() {
        let s = parse_probe("alpine_root=1\nalpine_runner=1\nalpine_release=3.22.1\n");
        assert!(s.not_ready().is_none());
        assert_eq!(image_for_release(s.release.as_deref().unwrap()).as_deref(), Some("docker.io/library/rust:1-alpine3.22"));
        assert!(parse_probe("alpine_runner=1\n").not_ready().unwrap().contains("bootstrap"));
        assert!(parse_probe("alpine_root=1\n").not_ready().unwrap().contains("alpine-run"));
        assert_eq!(image_for_release("edge"), None);
        assert_eq!(image_for_release("3"), None);
        // the probe runs under sh against an empty tree
        let o = std::process::Command::new("sh").arg("-c").arg(probe_cmd().replace(ROOT, "/nonexistent-root")).output().unwrap();
        assert!(o.status.success());
        assert_eq!(parse_probe(&String::from_utf8_lossy(&o.stdout)), RootState::default());
    }

    #[test]
    fn the_mirror_can_be_depot() {
        let m = Mirror { base: "http://depot.lan:8080/alpine/".into(), branch: "v3.22".into() };
        assert_eq!(m.repo("main"), "http://depot.lan:8080/alpine/v3.22/main");
        let _ = std::io::stdout().flush();
    }
}
