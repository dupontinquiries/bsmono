//! isolate.rs — fail-closed isolation policy for running untrusted code.
//!
//! Replaces the argv assembly in `sandbox.rs`. Same shape: pure string building, no process spawn,
//! unit-testable without bwrap installed. Four changes that matter:
//!
//!   1. FAIL CLOSED. `plan()` returns `Err(Refusal)` when the isolation a tier requires is not
//!      available. The old path ran the command anyway and set `sandboxed: false` — for
//!      model-generated code that is the wrong default, because the caller usually reads
//!      `passed` and not `sandboxed`.
//!   2. `--clearenv` + an explicit allowlist. Without it the sandbox inherits SSH_AUTH_SOCK,
//!      DBUS_SESSION_BUS_ADDRESS, DISPLAY and every *_TOKEN in the GUI's environment.
//!   3. `--unshare-pid/ipc/uts/user`, not just `--unshare-net`. Sharing the PID namespace at the
//!      same uid means the sandboxed build can ptrace or signal the process that launched it.
//!   4. CARGO_HOME is never writable for untrusted tiers, and a networked fetch writes to a
//!      separate cache that the host toolchain does not read.
//!
//! Not handled here (needs a broker process, not argv): /dev/dri leasing, egress allowlisting,
//! cgroup limits.

use std::path::{Path, PathBuf};

/// How much the caller trusts the code about to run. Drives everything else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trust {
    /// Code the human wrote and is iterating on. Isolation is a convenience, not a boundary.
    Own,
    /// Code a model produced this session. Assume mechanically broken and possibly hostile by
    /// accident — a hallucinated crate name that happens to be squatted is enough.
    Generated,
    /// Code or data of unknown provenance: a downloaded GGUF, a third-party repo, a corpus drop.
    Untrusted,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Net {
    /// No network namespace at all. The only honest option for `Untrusted`.
    None,
    /// Host network. Required for `cargo` to resolve a registry.
    Host,
}

/// Why a run was refused. Carries text fit to show in the UI verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoSandbox { need: &'static str },
    NetForbidden { trust: Trust },
    UnvettedDeps { deps: Vec<String> },
}

/// The marker every refusal message begins with.
///
/// `classify()` has only `(passed, output)` to work from, so a refusal has to be recognisable in
/// the text — and sniffing prose is brittle: the messages already start with two different phrasings
/// ("refusing to run", "refusing to fetch") and a wording change would silently reclassify a
/// refusal as a tool error, which is the one confusion that matters here. One constant, produced by
/// message() and consumed by classify(), makes an edit to either side a compile-time relationship.
pub const REFUSAL_MARK: &str = "[isolation refused]";

impl Refusal {
    /// Prefixed with [`REFUSAL_MARK`] so the classifier can tell "we did not run this" from "it ran
    /// and the toolchain complained" — different facts with different fixes.
    pub fn message(&self) -> String {
        format!("{REFUSAL_MARK} {}", self.reason())
    }

    fn reason(&self) -> String {
        match self {
            Refusal::NoSandbox { need } => format!(
                "refusing to run: {need} is not available and this code is not trusted. \
                 Install bubblewrap, or re-run with Trust::Own if you wrote this."
            ),
            Refusal::NetForbidden { trust } => format!(
                "refusing to run: network requested for {trust:?} code. \
                 Vet the dependencies and re-run as Trust::Generated, or pre-populate the cache."
            ),
            Refusal::UnvettedDeps { deps } => format!(
                "refusing to fetch unvetted crates: {}. A generated `use` line is a crate name \
                 chosen by a model; fetching it compiles that crate's build.rs on this machine.",
                deps.join(", ")
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub trust: Trust,
    pub net: Net,
    /// Read-only toolchain + registry cache. Never bound writable outside `Trust::Own`.
    pub cargo_home: PathBuf,
    /// Writable download cache used when a networked fetch is allowed for untrusted-ish code, so
    /// fetched crates never land in the host's `CARGO_HOME`.
    pub fetch_cache: Option<PathBuf>,
    /// Crate names the human has vetted. A `Generated` run may only fetch from this set.
    pub allowed_deps: Vec<String>,
}

impl Policy {
    pub fn for_trust(trust: Trust, cargo_home: impl Into<PathBuf>) -> Self {
        Self {
            trust,
            net: Net::None,
            cargo_home: cargo_home.into(),
            fetch_cache: None,
            allowed_deps: Vec::new(),
        }
    }
    pub fn with_net(mut self, net: Net) -> Self {
        self.net = net;
        self
    }
    pub fn allowing(mut self, deps: &[&str]) -> Self {
        self.allowed_deps = deps.iter().map(|s| s.to_string()).collect();
        self
    }
}

/// What the environment can actually provide. Constructed by the caller from real probes so
/// `plan()` stays pure and testable.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub bwrap: bool,
    pub userns: bool,
}

/// Environment variables allowed through `--clearenv`. Anything not here does not exist inside.
/// Deliberately excludes SSH_AUTH_SOCK, DISPLAY, WAYLAND_DISPLAY, DBUS_SESSION_BUS_ADDRESS,
/// XDG_RUNTIME_DIR, and every credential-shaped name.
const ENV_ALLOW: &[&str] = &["TERM", "LANG", "LC_ALL", "TZ", "RUSTFLAGS", "RUST_BACKTRACE"];

/// Curated /etc. Binding all of /etc leaks the host user list and every daemon config that happens
/// to be world-readable.
const ETC_ALLOW: &[&str] = &[
    "/etc/ssl",
    "/etc/pki",
    "/etc/ca-certificates",
    "/etc/ca-certificates.conf",
    "/etc/ld.so.cache",
    "/etc/ld.so.conf",
    "/etc/ld.so.conf.d",
    // Not optional: on Debian/Ubuntu `cc` is /usr/bin/cc -> /etc/alternatives/cc -> gcc. Drop this
    // and every link step fails with "linker `cc` not found" inside an otherwise working sandbox.
    "/etc/alternatives",
    "/etc/localtime",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub argv: Vec<String>,
    /// True only when real isolation is in force. Never true on a degraded path — there is no
    /// degraded path any more, but callers still branch on this for display.
    pub isolated: bool,
}

/// Is an unprivileged user namespace actually available?
///
/// bwrap being on PATH is not the same question: a kernel with
/// `kernel.unprivileged_userns_clone=0` (Debian's old default, and several hardened distros) has
/// the binary and refuses the syscall, so a capability check that only looks for the executable
/// reports isolation that will fail at spawn time. Reading the sysctl is cheap and answers the
/// question actually being asked. Absent file = the knob does not exist on this kernel, which
/// means unprivileged namespaces are permitted.
pub fn userns_available() -> bool {
    match std::fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone") {
        Ok(v) => v.trim() != "0",
        Err(_) => true,
    }
}

/// Build the argv, or refuse. `mirror` is the single writable path; `command` runs inside it.
pub fn plan(
    policy: &Policy,
    caps: Capabilities,
    mirror: &Path,
    command: &[String],
    detected_deps: &[String],
) -> Result<Plan, Refusal> {
    if policy.trust != Trust::Own && !(caps.bwrap && caps.userns) {
        return Err(Refusal::NoSandbox {
            need: if !caps.bwrap { "bubblewrap" } else { "unprivileged user namespaces" },
        });
    }
    if policy.net == Net::Host && policy.trust == Trust::Untrusted {
        return Err(Refusal::NetForbidden { trust: policy.trust });
    }
    if policy.net == Net::Host && policy.trust == Trust::Generated {
        let unvetted: Vec<String> = detected_deps
            .iter()
            .filter(|d| !policy.allowed_deps.iter().any(|a| a == *d))
            .cloned()
            .collect();
        if !unvetted.is_empty() {
            return Err(Refusal::UnvettedDeps { deps: unvetted });
        }
    }

    let m = mirror.display().to_string();
    let mut a: Vec<String> = vec!["bwrap".into()];
    let push = |args: &[&str], a: &mut Vec<String>| {
        for s in args {
            a.push((*s).to_string());
        }
    };

    push(
        &[
            "--unshare-user",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--hostname",
            "logscope-verify",
            "--die-with-parent",
            "--new-session",
            "--clearenv",
        ],
        &mut a,
    );
    if policy.net == Net::None {
        a.push("--unshare-net".into());
    }

    push(&["--ro-bind", "/usr", "/usr"], &mut a);
    for (link, at) in [("usr/bin", "/bin"), ("usr/sbin", "/sbin"), ("usr/lib", "/lib"), ("usr/lib64", "/lib64")] {
        a.push("--symlink".into());
        a.push(link.into());
        a.push(at.into());
    }
    a.push("--tmpfs".into());
    a.push("/etc".into());
    for p in ETC_ALLOW {
        a.push("--ro-bind-try".into());
        a.push((*p).into());
        a.push((*p).into());
    }
    if policy.net == Net::Host {
        // Only meaningful with a network namespace shared with the host. resolv.conf is often a
        // symlink into /run, so the target has to come along or DNS fails inside the mount ns.
        for p in ["/etc/resolv.conf", "/run/systemd/resolve", "/run/NetworkManager"] {
            a.push("--ro-bind-try".into());
            a.push(p.into());
            a.push(p.into());
        }
    }
    push(&["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--tmpfs", "/run"], &mut a);

    // Toolchain: read-only unless the human owns the code.
    let ch = policy.cargo_home.display().to_string();
    a.push(if policy.trust == Trust::Own { "--bind".into() } else { "--ro-bind".into() });
    a.push(ch.clone());
    a.push(ch.clone());

    // A networked fetch for non-Own code writes to a side cache, so a malicious build script
    // cannot leave anything in the registry the host toolchain will later compile.
    if policy.net == Net::Host && policy.trust != Trust::Own {
        if let Some(fc) = &policy.fetch_cache {
            let f = fc.display().to_string();
            a.push("--bind".into());
            a.push(f.clone());
            a.push(f.clone());
            a.push("--setenv".into());
            a.push("CARGO_HOME".into());
            a.push(f);
        }
    }

    a.push("--bind".into());
    a.push(m.clone());
    a.push(m.clone());
    a.push("--chdir".into());
    a.push(m.clone());
    a.push("--setenv".into());
    a.push("HOME".into());
    a.push(m.clone());
    a.push("--setenv".into());
    a.push("PATH".into());
    a.push("/usr/local/bin:/usr/bin:/bin".into());
    a.push("--setenv".into());
    a.push("CARGO_TARGET_DIR".into());
    a.push(format!("{m}/target"));
    for k in ENV_ALLOW {
        if let Ok(v) = std::env::var(k) {
            a.push("--setenv".into());
            a.push((*k).into());
            a.push(v);
        }
    }
    a.push("--remount-ro".into());
    a.push("/etc".into());
    a.push("--".into());
    a.extend(command.iter().cloned());

    Ok(Plan { argv: a, isolated: policy.trust != Trust::Own || caps.bwrap })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_ok() -> Capabilities {
        Capabilities { bwrap: true, userns: true }
    }
    fn cmd() -> Vec<String> {
        vec!["cargo".into(), "check".into(), "--offline".into()]
    }
    fn mirror() -> PathBuf {
        PathBuf::from("/tmp/lsv/ws-1")
    }

    fn argv_of(p: &Policy, caps: Capabilities) -> Vec<String> {
        plan(p, caps, &mirror(), &cmd(), &[]).expect("should plan").argv
    }
    fn has_pair(a: &[String], flag: &str, val: &str) -> bool {
        a.windows(2).any(|w| w[0] == flag && w[1] == val)
    }

    #[test]
    fn refuses_when_bwrap_missing_and_code_is_not_ours() {
        let p = Policy::for_trust(Trust::Generated, "/home/u/.cargo");
        let caps = Capabilities { bwrap: false, userns: true };
        assert!(matches!(
            plan(&p, caps, &mirror(), &cmd(), &[]),
            Err(Refusal::NoSandbox { .. })
        ));
    }

    #[test]
    fn refuses_when_userns_is_blocked() {
        let p = Policy::for_trust(Trust::Untrusted, "/home/u/.cargo");
        let caps = Capabilities { bwrap: true, userns: false };
        assert!(matches!(
            plan(&p, caps, &mirror(), &cmd(), &[]),
            Err(Refusal::NoSandbox { .. })
        ));
    }

    #[test]
    fn untrusted_never_gets_network() {
        let p = Policy::for_trust(Trust::Untrusted, "/home/u/.cargo").with_net(Net::Host);
        assert_eq!(
            plan(&p, caps_ok(), &mirror(), &cmd(), &[]),
            Err(Refusal::NetForbidden { trust: Trust::Untrusted })
        );
    }

    #[test]
    fn generated_code_cannot_fetch_a_crate_nobody_vetted() {
        let p = Policy::for_trust(Trust::Generated, "/home/u/.cargo")
            .with_net(Net::Host)
            .allowing(&["serde"]);
        let deps = vec!["serde".to_string(), "bevy_ecs_tilemap_hax".to_string()];
        match plan(&p, caps_ok(), &mirror(), &cmd(), &deps) {
            Err(Refusal::UnvettedDeps { deps }) => {
                assert_eq!(deps, vec!["bevy_ecs_tilemap_hax".to_string()])
            }
            other => panic!("expected refusal, got {:?}", other),
        }
    }

    #[test]
    fn vetted_deps_are_allowed_through() {
        let p = Policy::for_trust(Trust::Generated, "/home/u/.cargo")
            .with_net(Net::Host)
            .allowing(&["serde", "anyhow"]);
        let deps = vec!["serde".to_string(), "anyhow".to_string()];
        assert!(plan(&p, caps_ok(), &mirror(), &cmd(), &deps).is_ok());
    }

    #[test]
    fn host_environment_does_not_leak_in() {
        std::env::set_var("GITHUB_TOKEN", "ghp_secret");
        std::env::set_var("SSH_AUTH_SOCK", "/run/user/1000/keyring/ssh");
        let a = argv_of(&Policy::for_trust(Trust::Generated, "/home/u/.cargo"), caps_ok());
        assert!(a.contains(&"--clearenv".to_string()));
        assert!(!a.iter().any(|s| s.contains("ghp_secret")));
        assert!(!a.iter().any(|s| s == "SSH_AUTH_SOCK"));
        assert!(!a.iter().any(|s| s == "DBUS_SESSION_BUS_ADDRESS"));
    }

    #[test]
    fn namespaces_are_unshared_not_just_the_network() {
        let a = argv_of(&Policy::for_trust(Trust::Generated, "/home/u/.cargo"), caps_ok());
        for flag in ["--unshare-pid", "--unshare-ipc", "--unshare-user", "--new-session"] {
            assert!(a.contains(&flag.to_string()), "missing {}", flag);
        }
    }

    #[test]
    fn whole_etc_is_never_bound() {
        let a = argv_of(&Policy::for_trust(Trust::Generated, "/home/u/.cargo"), caps_ok());
        assert!(!has_pair(&a, "--ro-bind", "/etc"));
        assert!(has_pair(&a, "--tmpfs", "/etc"));
        assert!(has_pair(&a, "--remount-ro", "/etc"));
    }

    #[test]
    fn toolchain_is_read_only_for_generated_code() {
        let a = argv_of(&Policy::for_trust(Trust::Generated, "/home/u/.cargo"), caps_ok());
        assert!(has_pair(&a, "--ro-bind", "/home/u/.cargo"));
        assert!(!has_pair(&a, "--bind", "/home/u/.cargo"));
    }

    #[test]
    fn networked_fetch_writes_to_a_side_cache_not_the_host_registry() {
        let mut p = Policy::for_trust(Trust::Generated, "/home/u/.cargo")
            .with_net(Net::Host)
            .allowing(&["serde"]);
        p.fetch_cache = Some(PathBuf::from("/srv/codexbox/cache/fetch"));
        let a = plan(&p, caps_ok(), &mirror(), &cmd(), &["serde".to_string()])
            .unwrap()
            .argv;
        assert!(has_pair(&a, "--ro-bind", "/home/u/.cargo"));
        assert!(has_pair(&a, "--bind", "/srv/codexbox/cache/fetch"));
        assert!(has_pair(&a, "CARGO_HOME", "/srv/codexbox/cache/fetch"));
    }

    #[test]
    fn offline_run_unshares_the_network() {
        let a = argv_of(&Policy::for_trust(Trust::Generated, "/home/u/.cargo"), caps_ok());
        assert!(a.contains(&"--unshare-net".to_string()));
    }

    #[test]
    fn the_mirror_is_the_only_writable_real_path() {
        let a = argv_of(&Policy::for_trust(Trust::Generated, "/home/u/.cargo"), caps_ok());
        let binds: Vec<&String> = a
            .windows(2)
            .filter(|w| w[0] == "--bind")
            .map(|w| &w[1])
            .collect();
        assert_eq!(binds, vec![&"/tmp/lsv/ws-1".to_string()]);
    }
}
