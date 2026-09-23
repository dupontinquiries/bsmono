//! patchset — ship fixes to a package the fleet runs, freeze that version,
//! and bump it deliberately.
//!
//! The problem: cosmic-comp 1.8.0 on the phones needs two patches (touch
//! focus, the EI allowlist for tideshell's trackpad) and must not be
//! replaced by the next upstream apk behind our backs. Alpine's tagged
//! repositories solve both at once: gitmind serves a repo on the LAN, the
//! phone's `/etc/apk/repositories` carries it as `@fleet`, and the world
//! entry `cosmic-comp@fleet` means only that repo can ever supply the
//! package. Upstream moving on changes nothing. A "stable bump" is us
//! rebasing the patches onto a newer upstream, rebuilding, and publishing
//! the new apk to the same repo; the phone follows on its next
//! `apk upgrade`, and a failed boot rolls back on a timer.
//!
//! Everything here is pure: text in, APKBUILDs / world entries / plan steps
//! out. gitmind's runner executes the steps; gitmind-egui shows the
//! patchset next to what each device has installed (`apk info -v`), so the
//! drift between the frozen base and the fleet is visible.

use std::collections::BTreeMap;

/// One patch file, in apply order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// File name under the patchset directory (`0001-....patch`).
    pub file: String,
    /// One line, for the UI and the APKBUILD comment.
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestedOn {
    pub device: String,
    pub date: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patchset {
    /// Package name (also the aport name).
    pub pkg: String,
    /// The upstream version the patches are for ("1.8.0").
    pub base_version: String,
    /// The distro's pkgrel for that version; ours is one higher.
    pub base_pkgrel: u32,
    /// Upstream source tarball, with `{version}` substituted.
    pub source_url: String,
    /// What the repo tag is, e.g. "fleet".
    pub tag: String,
    /// How the package is built into an apk (the APKBUILD's per-package
    /// facts). Distinct per package: cosmic-comp's are not tideshell's.
    pub recipe: Recipe,
    pub patches: Vec<Patch>,
    pub tested_on: Vec<TestedOn>,
    pub notes: String,
}

/// The parts of an APKBUILD that differ between packages. The defaults suit
/// a plain Rust binary crate in a GitHub-style tarball; anything else says
/// so in its patchset.txt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// Project homepage (`url=`). Default: `source_url` up to `/archive/`.
    pub url: String,
    /// SPDX licence (`license=`). No sensible default: empty is reported by
    /// `show` and written as "unknown", which abuild accepts but a reviewer
    /// should not.
    pub license: String,
    /// The unpacked source directory under $srcdir, `{pkg}`/`{version}`
    /// substituted. Default `{pkg}-{version}`.
    pub builddir: String,
    /// The body of package(), one shell line per entry, `{pkg}` substituted.
    /// Default: install the one binary. A package that ships more than its
    /// binary MUST list it all: apk removes every file the old package owned
    /// that the new one does not (cosmic-comp owns its default keybindings).
    pub package: Vec<String>,
}

pub const DEFAULT_BUILDDIR: &str = "{pkg}-{version}";
pub const DEFAULT_PACKAGE: &str = "install -Dm755 target/release/{pkg} \"$pkgdir\"/usr/bin/{pkg}";

impl Recipe {
    fn defaults(source_url: &str) -> Recipe {
        let url = source_url.split("/archive/").next().unwrap_or(source_url).to_string();
        Recipe { url, license: String::new(), builddir: DEFAULT_BUILDDIR.into(), package: vec![DEFAULT_PACKAGE.into()] }
    }
}

impl Patchset {
    pub fn pkgrel(&self) -> u32 {
        self.base_pkgrel + 1
    }

    /// The source directory name under $srcdir.
    pub fn builddir(&self) -> String {
        self.recipe.builddir.replace("{pkg}", &self.pkg).replace("{version}", &self.base_version)
    }

    /// `1.8.0-r1`.
    pub fn version_string(&self) -> String {
        format!("{}-r{}", self.base_version, self.pkgrel())
    }

    /// The text form (`patchset.txt`): one key per line, patches and tests
    /// as repeated lines. Hand-rolled so the file stays readable in a
    /// terminal and diffable in git.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("pkg = {}\n", self.pkg));
        s.push_str(&format!("base_version = {}\n", self.base_version));
        s.push_str(&format!("base_pkgrel = {}\n", self.base_pkgrel));
        s.push_str(&format!("source_url = {}\n", self.source_url));
        s.push_str(&format!("tag = {}\n", self.tag));
        s.push_str(&format!("url = {}\n", self.recipe.url));
        if !self.recipe.license.is_empty() {
            s.push_str(&format!("license = {}\n", self.recipe.license));
        }
        s.push_str(&format!("builddir = {}\n", self.recipe.builddir));
        for l in &self.recipe.package {
            s.push_str(&format!("package = {l}\n"));
        }
        for p in &self.patches {
            s.push_str(&format!("patch = {} | {}\n", p.file, p.subject));
        }
        for t in &self.tested_on {
            s.push_str(&format!("tested = {} | {} | {}\n", t.device, t.date, t.note));
        }
        if !self.notes.is_empty() {
            s.push_str(&format!("notes = {}\n", self.notes.replace('\n', " ")));
        }
        s
    }

    pub fn from_text(text: &str) -> Result<Patchset, String> {
        let mut kv: BTreeMap<&str, String> = BTreeMap::new();
        let mut patches = Vec::new();
        let mut tested = Vec::new();
        let mut package: Vec<String> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| format!("line {}: expected key = value", n + 1))?;
            let (k, v) = (k.trim(), v.trim());
            match k {
                "patch" => {
                    let (f, subj) = v.split_once('|').ok_or_else(|| format!("line {}: patch = FILE | subject", n + 1))?;
                    patches.push(Patch { file: f.trim().into(), subject: subj.trim().into() });
                }
                "package" => package.push(v.to_string()),
                "tested" => {
                    let parts: Vec<&str> = v.splitn(3, '|').map(str::trim).collect();
                    if parts.len() < 2 {
                        return Err(format!("line {}: tested = DEVICE | DATE | note", n + 1));
                    }
                    tested.push(TestedOn { device: parts[0].into(), date: parts[1].into(), note: parts.get(2).map(|s| s.to_string()).unwrap_or_default() });
                }
                _ => {
                    kv.insert(k, v.to_string());
                }
            }
        }
        let get = |k: &str| kv.get(k).cloned().ok_or_else(|| format!("missing {k}"));
        let pkg = get("pkg")?;
        let base_version = get("base_version")?;
        let base_pkgrel = get("base_pkgrel")?.parse().map_err(|_| "base_pkgrel: not a number".to_string())?;
        let source_url = get("source_url")?;
        let d = Recipe::defaults(&source_url);
        let recipe = Recipe {
            url: kv.get("url").cloned().unwrap_or(d.url),
            license: kv.get("license").cloned().unwrap_or(d.license),
            builddir: kv.get("builddir").cloned().unwrap_or(d.builddir),
            package: if package.is_empty() { d.package } else { package },
        };
        Ok(Patchset {
            pkg,
            base_version,
            base_pkgrel,
            source_url,
            tag: kv.get("tag").cloned().unwrap_or_else(|| "fleet".into()),
            recipe,
            patches,
            tested_on: tested,
            notes: kv.get("notes").cloned().unwrap_or_default(),
        })
    }

    /// A stable bump: the same patches on a newer upstream. Tests are
    /// cleared because nothing has been tested on it yet; the rollout plan
    /// refuses to reach a device until `git apply --check` passed on the
    /// builder and a device is recorded as tested.
    pub fn bump(&self, new_version: &str, new_base_pkgrel: u32) -> Patchset {
        Patchset { base_version: new_version.into(), base_pkgrel: new_base_pkgrel, tested_on: Vec::new(), ..self.clone() }
    }

    pub fn is_tested(&self) -> bool {
        !self.tested_on.is_empty()
    }

    /// The upstream tarball URL for this version.
    pub fn source(&self) -> String {
        self.source_url.replace("{version}", &self.base_version)
    }
}

/// An APKBUILD that builds the package from upstream source with the
/// patches applied. abuild's `default_prepare` applies every `*.patch` in
/// `source=` in order, which is why the patch files are listed there.
/// `options="net"` lets cargo fetch in `build()`, the way the pmOS aports
/// for COSMIC do.
pub fn apkbuild_source(ps: &Patchset, arch: &str, build_deps: &[&str]) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Maintainer: gitmind fleet (patchset {}: {} patch(es) on upstream {})\n", ps.pkg, ps.patches.len(), ps.base_version));
    for p in &ps.patches {
        s.push_str(&format!("#   {}: {}\n", p.file, p.subject));
    }
    s.push_str(&format!("pkgname={}\n", ps.pkg));
    s.push_str(&format!("pkgver={}\n", ps.base_version));
    s.push_str(&format!("pkgrel={}\n", ps.pkgrel()));
    s.push_str(&format!("pkgdesc=\"{} {} with the fleet patchset\"\n", ps.pkg, ps.base_version));
    s.push_str(&format!("url=\"{}\"\n", ps.recipe.url));
    s.push_str(&format!("arch=\"{arch}\"\n"));
    s.push_str(&format!("license=\"{}\"\n", if ps.recipe.license.is_empty() { "unknown" } else { &ps.recipe.license }));
    s.push_str("options=\"net !check\"\n");
    s.push_str(&format!("makedepends=\"{}\"\n", build_deps.join(" ")));
    let mut sources = vec![format!("{}-{}.tar.gz::{}", ps.pkg, ps.base_version, ps.source())];
    sources.extend(ps.patches.iter().map(|p| p.file.clone()));
    s.push_str(&format!("source=\"{}\"\n", sources.join("\n\t")));
    s.push_str(&format!("builddir=\"$srcdir/{}\"\n\n", ps.builddir()));
    s.push_str("prepare() {\n\tdefault_prepare\n\tcargo fetch --locked\n}\n\n");
    s.push_str("build() {\n\tcargo build --release --frozen\n}\n\n");
    let body: Vec<String> = ps.recipe.package.iter().map(|l| format!("\t{}", l.replace("{pkg}", &ps.pkg))).collect();
    s.push_str(&format!("package() {{\n{}\n}}\n", body.join("\n")));
    s.push_str("sha512sums=\"SKIP\"\n");
    s
}

/// The freeze on a device: the repo line and the world entries that pin
/// each package to the tagged repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Freeze {
    pub repo_url: String,
    pub tag: String,
    /// Packages that must only ever come from the tagged repo.
    pub pinned: Vec<String>,
}

impl Freeze {
    /// `@fleet http://gitmind.lan/apk` for /etc/apk/repositories.
    pub fn repositories_line(&self) -> String {
        format!("@{} {}", self.tag, self.repo_url.trim_end_matches('/'))
    }

    /// `cosmic-comp@fleet` entries for /etc/apk/world; `apk add` of these
    /// replaces the untagged entries, and from then on upstream cannot
    /// supply the package.
    pub fn world_entries(&self) -> Vec<String> {
        self.pinned.iter().map(|p| format!("{p}@{}", self.tag)).collect()
    }

    /// The one command that installs the freeze (repo line, then the pins).
    pub fn install_cmd(&self) -> String {
        format!(
            "grep -qxF '{line}' /etc/apk/repositories || echo '{line}' >> /etc/apk/repositories; apk update && apk add --allow-untrusted {pins}",
            line = self.repositories_line(),
            pins = self.world_entries().join(" ")
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    Builder,
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    ExitZero,
    /// The step's output must contain this.
    Contains(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub on: Where,
    pub what: &'static str,
    pub cmd: String,
    pub gate: Gate,
    /// Run on the device if the step's gate fails (or a later one does).
    pub rollback: Option<String>,
}

/// Where the device keeps the package it had before a rollout.
pub fn rollback_dir(pkg: &str) -> String {
    format!("/var/lib/fleet/rollback/{pkg}")
}

/// Put the kept package back. NOT `apk fix`: that reinstalls whatever the
/// world selects, and after the freeze the world says `pkg@fleet`, which is
/// the new build. Installing the kept file restores the old version and
/// replaces the @fleet world entry, so the tagged repo stops supplying it.
pub fn revert_cmd(pkg: &str) -> String {
    format!("apk add --allow-untrusted {}/*.apk", rollback_dir(pkg))
}

/// Save the currently installed package file under rollback_dir. The
/// version comes from `apk info -v` ("cosmic-comp-1.8.0-r0" -> "=1.8.0-r0").
/// Earlier saves are cleared first so the directory holds exactly one apk.
pub fn keep_cmd(pkg: &str) -> String {
    let d = rollback_dir(pkg);
    format!("mkdir -p {d} && rm -f {d}/*.apk && cd {d} && apk fetch {pkg}=$(apk info -v {pkg} | head -n 1 | sed 's/^{pkg}-//') && ls {d}/*.apk")
}

/// Seconds the dead-man timer gives the new compositor to come up before
/// the device reverts on its own.
pub const DEAD_MAN_S: u32 = 180;

/// The rollout: build on the builder, publish, then on the device install
/// under a dead-man timer, restart the compositor, confirm, disarm.
/// Refuses (returns Err) a bump nobody has tested.
pub fn rollout(ps: &Patchset, freeze: &Freeze, builder_repo_dir: &str, require_tested: bool) -> Result<Vec<Step>, String> {
    if require_tested && !ps.is_tested() {
        return Err(format!("{} {} has no tested_on entry: build and test it on one device (require_tested=false) before the fleet", ps.pkg, ps.version_string()));
    }
    let apk = format!("{}-{}.apk", ps.pkg, ps.version_string());
    let mut steps = vec![
        Step { on: Where::Builder, what: "fetch upstream", cmd: format!("cd {d}/{p} && abuild fetch", d = builder_repo_dir, p = ps.pkg), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Builder, what: "patches apply cleanly", cmd: format!("cd {d}/{p} && abuild unpack && cd src/{b} && for f in {patches}; do git apply --check ../../$f || exit 1; done", d = builder_repo_dir, p = ps.pkg, b = ps.builddir(), patches = ps.patches.iter().map(|p| p.file.as_str()).collect::<Vec<_>>().join(" ")), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Builder, what: "build the apk", cmd: format!("cd {d}/{p} && abuild -r -F", d = builder_repo_dir, p = ps.pkg), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Builder, what: "publish to the fleet repo", cmd: format!("cd {d}/repo && apk index -o APKINDEX.tar.gz *.apk && ls {apk}", d = builder_repo_dir), gate: Gate::Contains(apk.clone()), rollback: None },
        Step { on: Where::Device, what: "record the installed version", cmd: format!("apk info -v {} | head -n 1", ps.pkg), gate: Gate::Contains(ps.pkg.clone()), rollback: None },
        // The old package, kept on the device, is what every rollback
        // reinstalls. `apk fetch NAME=VERSION` asks the repos the device
        // installed it from; the gate is that a file actually landed.
        Step { on: Where::Device, what: "keep the installed package for rollback", cmd: keep_cmd(&ps.pkg), gate: Gate::Contains(".apk".into()), rollback: None },
        Step { on: Where::Device, what: "arm the dead-man timer", cmd: format!("systemd-run --unit=fleet-revert-{p} --on-active={s} sh -c '{r} ; systemctl restart display-manager'", p = ps.pkg, s = DEAD_MAN_S, r = revert_cmd(&ps.pkg)), gate: Gate::ExitZero, rollback: Some(format!("systemctl stop fleet-revert-{}.timer", ps.pkg)) },
        Step { on: Where::Device, what: "install the freeze and the patched package", cmd: freeze.install_cmd(), gate: Gate::ExitZero, rollback: Some(revert_cmd(&ps.pkg)) },
        Step { on: Where::Device, what: "the new version is installed", cmd: format!("apk info -v {}", ps.pkg), gate: Gate::Contains(format!("{}-{}", ps.pkg, ps.version_string())), rollback: Some(revert_cmd(&ps.pkg)) },
        Step { on: Where::Device, what: "restart the compositor", cmd: "systemctl restart display-manager".into(), gate: Gate::ExitZero, rollback: None },
        Step { on: Where::Device, what: "the compositor is up again", cmd: format!("sleep 20; pgrep -x {} && journalctl -b --no-pager -u display-manager -n 5", ps.pkg), gate: Gate::Contains("".into()), rollback: None },
        Step { on: Where::Device, what: "disarm the dead-man timer", cmd: format!("systemctl stop fleet-revert-{}.timer", ps.pkg), gate: Gate::ExitZero, rollback: None },
    ];
    // The compositor-up gate: pgrep's exit code is what matters.
    if let Some(up) = steps.iter_mut().find(|s| s.what == "the compositor is up again") {
        up.gate = Gate::ExitZero;
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosmic() -> Patchset {
        Patchset {
            pkg: "cosmic-comp".into(),
            base_version: "1.8.0".into(),
            base_pkgrel: 0,
            source_url: "https://github.com/pop-os/cosmic-comp/archive/refs/tags/epoch-{version}.tar.gz".into(),
            tag: "fleet".into(),
            recipe: Recipe {
                url: "https://github.com/pop-os/cosmic-comp".into(),
                license: "GPL-3.0-only".into(),
                builddir: "{pkg}-epoch-{version}".into(),
                package: vec!["make install DESTDIR=\"$pkgdir\" prefix=/usr".into()],
            },
            patches: vec![
                Patch { file: "0001-input-focus-element-on-first-touch-down.patch".into(), subject: "touch down focuses the element".into() },
                Patch { file: "0002-dbus-ei-allow-tideshell.patch".into(), subject: "EI: allow org.tideshell.Shell".into() },
            ],
            tested_on: vec![TestedOn { device: "fp5".into(), date: "2026-09-23".into(), note: "nested + phone".into() }],
            notes: "the FP5 fleet base".into(),
        }
    }

    #[test]
    fn text_form_round_trips_and_defaults_the_tag() {
        let ps = cosmic();
        let text = ps.to_text();
        let back = Patchset::from_text(&text).unwrap();
        assert_eq!(back, ps);
        let minimal = "pkg = x\nbase_version = 1\nbase_pkgrel = 0\nsource_url = u\n";
        assert_eq!(Patchset::from_text(minimal).unwrap().tag, "fleet");
        assert!(Patchset::from_text("pkg = x\n").unwrap_err().contains("missing base_version"));
        assert!(Patchset::from_text("junk line\n").is_err());
    }

    #[test]
    fn the_apkbuild_carries_the_patches_and_our_pkgrel() {
        let ps = cosmic();
        let a = apkbuild_source(&ps, "aarch64", &["cargo", "libinput-dev"]);
        assert!(a.contains("pkgver=1.8.0\n") && a.contains("pkgrel=1\n"), "one above the distro's -r0");
        assert!(a.contains("0001-input-focus-element-on-first-touch-down.patch\n\t0002-dbus-ei-allow-tideshell.patch\""), "patches in source=, in order");
        assert!(a.contains("epoch-1.8.0.tar.gz"), "the version substituted into the url");
        assert!(a.contains("default_prepare"), "abuild applies the patches");
        assert!(a.contains("options=\"net !check\"") && a.contains("--frozen"));
        assert!(a.contains("builddir=\"$srcdir/cosmic-comp-epoch-1.8.0\"") && a.contains("url=\"https://github.com/pop-os/cosmic-comp\""));
        // cosmic-comp's package owns its default keybindings and tiling
        // exceptions: a binary-only package() would make apk delete them.
        assert!(a.contains("package() {\n\tmake install DESTDIR=\"$pkgdir\" prefix=/usr\n}"), "{a}");
        assert_eq!(ps.version_string(), "1.8.0-r1");
    }

    #[test]
    fn the_fleets_cosmic_comp_patchset_packages_everything_make_install_does() {
        let ps = Patchset::from_text(include_str!("../../../../../system/patchsets/cosmic-comp/patchset.txt")).unwrap();
        let a = apkbuild_source(&ps, "aarch64", &["cargo"]);
        assert!(a.contains("make install DESTDIR=\"$pkgdir\" prefix=/usr"), "the real patchset must not package the binary alone");
        assert!(a.contains("builddir=\"$srcdir/cosmic-comp-epoch-1.8.0\"") && a.contains("license=\"GPL-3.0-only\""));
    }

    #[test]
    fn a_plain_recipe_defaults_and_nothing_is_cosmic_specific() {
        let t = "pkg = tideshell\nbase_version = 0.3.0\nbase_pkgrel = 0\nsource_url = https://example.org/tideshell/archive/v{version}.tar.gz\n";
        let ps = Patchset::from_text(t).unwrap();
        assert_eq!(ps.recipe.url, "https://example.org/tideshell");
        assert_eq!(ps.builddir(), "tideshell-0.3.0");
        let a = apkbuild_source(&ps, "aarch64", &["cargo"]);
        assert!(!a.contains("cosmic") && !a.contains("GPL"), "{a}");
        assert!(a.contains("license=\"unknown\"") && a.contains("install -Dm755 target/release/tideshell \"$pkgdir\"/usr/bin/tideshell"));
        assert_eq!(Patchset::from_text(&ps.to_text()).unwrap(), ps, "defaults round-trip");
        assert_eq!(ps.version_string(), "0.3.0-r1");
    }

    #[test]
    fn a_bump_keeps_the_patches_drops_the_tests_and_is_refused_until_tested() {
        let ps = cosmic();
        let b = ps.bump("1.9.0", 0);
        assert_eq!(b.patches, ps.patches);
        assert!(!b.is_tested() && b.version_string() == "1.9.0-r1" && b.source().contains("epoch-1.9.0"));
        let f = Freeze { repo_url: "http://gitmind.lan/apk/".into(), tag: "fleet".into(), pinned: vec!["cosmic-comp".into()] };
        assert!(rollout(&b, &f, "/srv/aports", true).unwrap_err().contains("no tested_on"));
        assert!(rollout(&b, &f, "/srv/aports", false).is_ok(), "one device first, with the check off");
        assert!(rollout(&ps, &f, "/srv/aports", true).is_ok());
    }

    #[test]
    fn the_freeze_pins_through_the_tagged_repo() {
        let f = Freeze { repo_url: "http://gitmind.lan/apk/".into(), tag: "fleet".into(), pinned: vec!["cosmic-comp".into(), "tideshell".into()] };
        assert_eq!(f.repositories_line(), "@fleet http://gitmind.lan/apk");
        assert_eq!(f.world_entries(), vec!["cosmic-comp@fleet", "tideshell@fleet"]);
        let c = f.install_cmd();
        assert!(c.contains("/etc/apk/repositories") && c.contains("apk add --allow-untrusted cosmic-comp@fleet tideshell@fleet"));
    }

    #[test]
    fn the_rollout_checks_patches_before_building_and_arms_the_timer_before_touching_the_device() {
        let ps = cosmic();
        let f = Freeze { repo_url: "http://gitmind.lan/apk".into(), tag: "fleet".into(), pinned: vec!["cosmic-comp".into()] };
        let steps = rollout(&ps, &f, "/srv/aports", true).unwrap();
        let idx = |what: &str| steps.iter().position(|s| s.what == what).unwrap_or_else(|| panic!("no step {what}"));
        assert!(idx("patches apply cleanly") < idx("build the apk"));
        assert!(idx("publish to the fleet repo") < idx("arm the dead-man timer"));
        assert!(idx("arm the dead-man timer") < idx("install the freeze and the patched package"));
        assert!(idx("restart the compositor") < idx("the compositor is up again"));
        assert!(idx("the compositor is up again") < idx("disarm the dead-man timer"));
        assert!(steps.iter().take_while(|s| s.on == Where::Builder).count() == 4, "the builder finishes before any device step");
        let install = &steps[idx("the new version is installed")];
        assert_eq!(install.gate, Gate::Contains("cosmic-comp-1.8.0-r1".into()));
        let rb = install.rollback.as_deref().unwrap();
        assert_eq!(rb, "apk add --allow-untrusted /var/lib/fleet/rollback/cosmic-comp/*.apk", "rollback reinstalls the kept package");
        assert!(!steps.iter().any(|s| s.cmd.contains("apk fix") || s.rollback.as_deref().is_some_and(|r| r.contains("apk fix"))), "apk fix reinstalls the @fleet build, it does not revert");
        assert!(idx("keep the installed package for rollback") < idx("arm the dead-man timer"), "the old package is on disk before anything can need it");
        let keep = &steps[idx("keep the installed package for rollback")].cmd;
        assert!(keep.contains("apk fetch cosmic-comp=$(") && keep.contains("sed 's/^cosmic-comp-//'"), "{keep}");
        assert!(steps[idx("arm the dead-man timer")].cmd.contains(&revert_cmd("cosmic-comp")));
        assert!(steps[idx("arm the dead-man timer")].cmd.contains(&format!("--on-active={DEAD_MAN_S}")));
    }
}
