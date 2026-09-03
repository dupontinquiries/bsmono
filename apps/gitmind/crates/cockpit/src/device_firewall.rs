//! Opening a port on the DEVICE, over ssh.
//!
//! Separate from the desktop-side story. `snapclone`'s `firewall_backend`
//! models opening a port on the machine the app is running on — firewalld over
//! D-Bus, ufw, an XDG portal inside a Flatpak. This is the other direction:
//! gitmind runs on a desktop and needs a port open on a phone it reaches by
//! ssh, so every option here is a shell command and none of them is D-Bus.
//!
//! It also has to know a tool that list does not: **awall**. Alpine and
//! postmarketOS use it, and it is the one that punishes a naive fix.
//!
//! # Why awall cannot be handled like the others
//!
//! `ufw allow 9876/tcp` and `nft add rule ...` change the running ruleset and
//! persist by their own mechanisms. awall does not work that way: it COMPILES a
//! set of JSON policies into the kernel ruleset, and `awall activate` replaces
//! whatever is loaded with the compiled result.
//!
//! So `iptables -I INPUT -p tcp --dport 9876 -j ACCEPT` on an awall system
//! works, and then silently stops working — at the next `awall activate`, at
//! the next boot, or when some unrelated change is applied. The operator sees a
//! share that worked yesterday and does not today, with nothing having changed
//! on the desktop.
//!
//! The right shape is a policy file in `/etc/awall/optional`, enabled and
//! activated. That is three commands rather than one, and it survives.
//!
//! # Nothing here runs anything
//!
//! Every function returns a command string. `deploy::remote_capture` runs it,
//! the caller can print it for a dry run, and the whole module is testable
//! without a device — which matters, because none of these tools is installed
//! in the sandbox this was written in.

/// A firewall gitmind knows how to ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFirewall {
    /// Alpine / postmarketOS. Declarative; see the module note.
    Awall,
    /// nftables directly — postmarketOS without awall, and most modern distros.
    Nftables,
    /// ufw, on a Debian-derived device.
    Ufw,
    /// firewalld, on a Fedora-derived device.
    Firewalld,
    /// iptables with no manager in front of it.
    Iptables,
    /// Nothing detected. NOT the same as "no firewall": a tool this does not
    /// know is indistinguishable from none, and saying so is better than
    /// claiming the port is open.
    Unknown,
}

/// One command whose output [`parse_detect`] reads.
///
/// Prints a line per tool that exists. `command -v` rather than `which`:
/// busybox has no `which` on a minimal Alpine install, and a missing `which`
/// would report every firewall as absent — which reads as "no firewall" and is
/// the most dangerous wrong answer this module can give.
pub fn detect_cmd() -> String {
    "for t in awall nft ufw firewall-cmd iptables; do \
     command -v \"$t\" >/dev/null 2>&1 && echo \"have $t\"; done; echo done"
        .to_string()
}

/// Which firewall to use, from [`detect_cmd`]'s output.
///
/// Ordered by specificity, not by preference: awall wins over nft because on
/// an Alpine device BOTH are present — awall is a front end for the same
/// kernel tables — and using the lower-level one there is exactly the mistake
/// the module note describes.
pub fn parse_detect(output: &str) -> DeviceFirewall {
    let has = |t: &str| output.lines().any(|l| l.trim() == format!("have {t}"));
    if has("awall") {
        DeviceFirewall::Awall
    } else if has("ufw") {
        DeviceFirewall::Ufw
    } else if has("firewall-cmd") {
        DeviceFirewall::Firewalld
    } else if has("nft") {
        DeviceFirewall::Nftables
    } else if has("iptables") {
        DeviceFirewall::Iptables
    } else {
        DeviceFirewall::Unknown
    }
}

/// The awall policy that opens `port`, as JSON.
///
/// A named policy rather than an edit to an existing one: gitmind's rule
/// should be removable without touching anything the operator wrote, and a
/// file it owns is the only way to promise that.
pub fn awall_policy(port: u16) -> String {
    format!(
        "{{\n  \"description\": \"gitmind share (tcp/{port})\",\n  \
         \"filter\": [\n    {{\n      \"in\": \"_fw\",\n      \"service\": \
         {{ \"proto\": \"tcp\", \"port\": {port} }},\n      \"action\": \"accept\"\n    \
         }}\n  ]\n}}\n"
    )
}

/// The command that opens `port` on this kind of device.
///
/// `None` for [`DeviceFirewall::Unknown`] — a guess would be a command that
/// fails, and a failing command in a bootstrap script is worse than an honest
/// "I do not know what firewall this device runs".
pub fn open_cmd(fw: DeviceFirewall, port: u16) -> Option<String> {
    Some(match fw {
        DeviceFirewall::Awall => format!(
            // Written, enabled, activated. All three, because a policy file
            // alone does nothing, and `enable` without `activate` only takes
            // effect at the next boot — which on a phone is far enough away
            // that the operator concludes it did not work.
            "mkdir -p /etc/awall/optional && \
             cat > /etc/awall/optional/gitmind-share.json <<'GITMIND_AWALL'\n{policy}GITMIND_AWALL\n\
             awall enable gitmind-share && awall activate -f",
            policy = awall_policy(port)
        ),
        // `--permanent` AND a reload: without the flag it is gone at reboot,
        // and without the reload it does not apply now. Both halves are the
        // usual mistake.
        DeviceFirewall::Firewalld => format!(
            "firewall-cmd --permanent --add-port={port}/tcp && firewall-cmd --reload"
        ),
        DeviceFirewall::Ufw => format!("ufw allow {port}/tcp"),
        DeviceFirewall::Nftables => format!(
            // `add table`/`add chain` are idempotent, so this is safe to run
            // twice — which a bootstrap script will do.
            "nft add table inet gitmind 2>/dev/null; \
             nft add chain inet gitmind input '{{ type filter hook input priority 0; }}' 2>/dev/null; \
             nft add rule inet gitmind input tcp dport {port} accept"
        ),
        // `-C` first so a rerun does not stack duplicate rules; `||` runs the
        // insert only when the check says it is absent.
        DeviceFirewall::Iptables => format!(
            "iptables -C INPUT -p tcp --dport {port} -j ACCEPT 2>/dev/null || \
             iptables -I INPUT -p tcp --dport {port} -j ACCEPT"
        ),
        DeviceFirewall::Unknown => return None,
    })
}

/// What to tell the operator when the firewall is not one we know.
pub fn unknown_advice(port: u16) -> String {
    format!(
        "could not identify a firewall on the device. If the share is refused, \
         allow tcp/{port} there by hand — on postmarketOS that is usually awall \
         (`awall list`), otherwise `nft list ruleset`."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn awall_wins_on_a_device_that_has_both() {
        // An Alpine device has awall AND nft — awall is a front end for the
        // same kernel tables. Choosing nft there writes a rule that
        // `awall activate` silently discards, which is the module's whole
        // warning.
        let out = "have awall\nhave nft\nhave iptables\ndone";
        assert_eq!(parse_detect(out), DeviceFirewall::Awall);
    }

    #[test]
    fn each_tool_is_recognised_on_its_own() {
        for (line, want) in [
            ("have nft\ndone", DeviceFirewall::Nftables),
            ("have ufw\ndone", DeviceFirewall::Ufw),
            ("have firewall-cmd\ndone", DeviceFirewall::Firewalld),
            ("have iptables\ndone", DeviceFirewall::Iptables),
        ] {
            assert_eq!(parse_detect(line), want, "for {line:?}");
        }
    }

    #[test]
    fn no_tools_is_unknown_rather_than_no_firewall() {
        // A firewall this does not know is indistinguishable from none, and
        // claiming the port is open is the most dangerous wrong answer here.
        assert_eq!(parse_detect("done"), DeviceFirewall::Unknown);
        assert_eq!(parse_detect(""), DeviceFirewall::Unknown);
        assert_eq!(open_cmd(DeviceFirewall::Unknown, 9876), None);
        assert!(unknown_advice(9876).contains("9876"));
    }

    #[test]
    fn a_partial_word_does_not_count_as_a_tool() {
        // Line-exact, not substring: `have nftables-helper` is not `nft`, and
        // a loose match would pick a firewall the device does not have and
        // then run a command that fails.
        assert_eq!(parse_detect("have nftables-helper\ndone"), DeviceFirewall::Unknown);
        assert_eq!(parse_detect("have ufw-frontend\ndone"), DeviceFirewall::Unknown);
    }

    #[test]
    fn the_awall_command_writes_enables_and_activates() {
        // A policy file alone does nothing; `enable` without `activate` only
        // takes effect at the next boot, which on a phone is far enough away
        // that the operator concludes it did not work.
        let c = open_cmd(DeviceFirewall::Awall, 9876).unwrap();
        let i = |s: &str| c.find(s).unwrap_or_else(|| panic!("missing {s} in {c}"));
        assert!(i("/etc/awall/optional/gitmind-share.json") < i("awall enable"));
        assert!(i("awall enable") < i("awall activate"));
        // And the policy itself names the port, not just the description.
        assert!(awall_policy(9876).contains("\"port\": 9876"), "{}", awall_policy(9876));
    }

    #[test]
    fn firewalld_is_both_permanent_and_reloaded() {
        // Without `--permanent` it is gone at reboot; without the reload it
        // does not apply now. Each half alone is the usual mistake.
        let c = open_cmd(DeviceFirewall::Firewalld, 9876).unwrap();
        assert!(c.contains("--permanent"), "{c}");
        assert!(c.contains("--add-port=9876/tcp"), "{c}");
        assert!(c.contains("--reload"), "{c}");
    }

    #[test]
    fn rerunning_does_not_stack_duplicate_rules() {
        // A bootstrap script runs more than once. iptables checks with `-C`
        // first; nftables' table and chain creation is idempotent.
        let ipt = open_cmd(DeviceFirewall::Iptables, 9876).unwrap();
        assert!(ipt.contains("-C INPUT"), "no existence check: {ipt}");
        assert!(ipt.find("-C INPUT").unwrap() < ipt.find("-I INPUT").unwrap());

        let nft = open_cmd(DeviceFirewall::Nftables, 9876).unwrap();
        assert!(nft.contains("add table"), "{nft}");
        assert!(nft.contains("2>/dev/null"), "table creation is not tolerant of a rerun: {nft}");
    }

    #[test]
    fn detection_does_not_depend_on_which() {
        // busybox on a minimal Alpine install has no `which`, and a missing
        // `which` reports every firewall as absent — which reads as "no
        // firewall", the worst wrong answer available.
        let c = detect_cmd();
        assert!(c.contains("command -v"), "{c}");
        assert!(!c.contains("which "), "{c}");
        assert!(c.contains("awall"), "the postmarketOS tool is not probed: {c}");
    }
}
