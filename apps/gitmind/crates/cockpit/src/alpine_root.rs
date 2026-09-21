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

fn valid_pkg(p: &str) -> bool {
    !p.is_empty() && p.len() <= 128 && p.chars().all(|c| c.is_ascii_alphanumeric() || "+-._".contains(c))
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
python3 -c 'import tarfile;tarfile.open("APKINDEX.tar.gz").extract("APKINDEX",".",filter="data")'
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
        std::fs::write(main.join("APKINDEX.tar.gz"), tgz(&[("APKINDEX", index.as_bytes(), 0o644)])).unwrap();
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
    fn the_mirror_can_be_depot() {
        let m = Mirror { base: "http://depot.lan:8080/alpine/".into(), branch: "v3.22".into() };
        assert_eq!(m.repo("main"), "http://depot.lan:8080/alpine/v3.22/main");
        let _ = std::io::stdout().flush();
    }
}
