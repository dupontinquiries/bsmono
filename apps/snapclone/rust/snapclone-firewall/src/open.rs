//! Port-opening implementations for each backend.
//!
//! Each function runs a subprocess with the appropriate firewall command.
//! pkexec / sudo elevation is used when the process lacks NET_ADMIN capability.
//! Mirrors FirewallPortManager::openPort() in C++.

use crate::{detect::{Backend, has_caps}, rule::{FirewallRule, Protocol}};
use anyhow::{bail, Result};
use tracing::{debug, info};

/// Open the firewall port for the given rule using the detected backend.
pub fn open_port(backend: Backend, rule: &FirewallRule) -> Result<()> {
    match backend {
        Backend::Firewalld => open_firewalld(rule),
        Backend::Ufw       => open_ufw(rule),
        Backend::Nftables  => open_nftables(rule),
        Backend::Iptables  => open_iptables(rule),
        Backend::None      => {
            info!("No firewall — port {} already accessible", rule.port);
            Ok(())
        }
    }
}

fn open_firewalld(rule: &FirewallRule) -> Result<()> {
    for proto in protocols(rule.protocol) {
        let port_proto = format!("{}/{}", rule.port, proto);
        let args: Vec<&str> = vec![
            "firewall-cmd", "--permanent",
            "--add-port", &port_proto,
        ];
        run_maybe_elevated(&args, "firewall-cmd")?;
    }
    // Reload to apply permanent changes
    run_maybe_elevated(&["firewall-cmd", "--reload"], "firewall-cmd")?;
    info!("firewalld: opened port {}", rule.port);
    Ok(())
}

fn open_ufw(rule: &FirewallRule) -> Result<()> {
    for proto in protocols(rule.protocol) {
        let port_proto = format!("{}/{}", rule.port, proto);
        let args: Vec<&str> = vec!["ufw", "allow", &port_proto, "comment", &rule.comment];
        run_maybe_elevated(&args, "ufw")?;
    }
    info!("ufw: opened port {}", rule.port);
    Ok(())
}

fn open_nftables(rule: &FirewallRule) -> Result<()> {
    // nft add rule inet filter input ... accept
    // This appends to the existing filter input chain (common on Alpine/postmarketOS)
    for proto in protocols(rule.protocol) {
        let nft_rule = format!(
            "add rule inet filter input {} dport {} accept comment \"{}\"",
            proto, rule.port, rule.comment
        );
        let args: Vec<&str> = vec!["nft", &nft_rule];
        run_maybe_elevated(&args, "nft")?;
    }
    info!("nftables: opened port {}", rule.port);
    Ok(())
}

fn open_iptables(rule: &FirewallRule) -> Result<()> {
    for proto in protocols(rule.protocol) {
        let port_str = rule.port.to_string();
        let args: Vec<&str> = vec![
            "iptables", "-I", "INPUT", "-p", proto,
            "--dport", &port_str, "-j", "ACCEPT",
            "-m", "comment", "--comment", &rule.comment,
        ];
        run_maybe_elevated(&args, "iptables")?;
    }
    info!("iptables: opened port {}", rule.port);
    Ok(())
}

/// Run a command, elevating with pkexec or sudo if we lack NET_ADMIN capability.
fn run_maybe_elevated(args: &[&str], _binary: &str) -> Result<()> {
    debug!("Firewall cmd: {:?}", args);

    let (program, cmd_args): (&str, Vec<&str>) = if has_caps() {
        (args[0], args[1..].to_vec())
    } else if binary_on_path("pkexec") {
        // pkexec pops a PolicyKit GUI — preferred on desktop
        ("pkexec", args.to_vec())
    } else {
        // sudo as last resort — may prompt on terminal
        ("sudo", args.to_vec())
    };

    let status = std::process::Command::new(program)
        .args(&cmd_args)
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run {}: {}", program, e))?;

    if !status.success() {
        bail!("{} exited with status {}", program, status);
    }
    Ok(())
}

fn protocols(p: Protocol) -> &'static [&'static str] {
    match p {
        Protocol::Tcp  => &["tcp"],
        Protocol::Udp  => &["udp"],
        Protocol::Both => &["tcp", "udp"],
    }
}

fn binary_on_path(name: &str) -> bool {
    std::process::Command::new("which").arg(name)
        .output().map(|o| o.status.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocols_expands_both_to_tcp_and_udp() {
        assert_eq!(protocols(Protocol::Tcp), &["tcp"]);
        assert_eq!(protocols(Protocol::Udp), &["udp"]);
        assert_eq!(protocols(Protocol::Both), &["tcp", "udp"]);
    }

    #[test]
    fn none_backend_is_a_noop_ok() {
        // Backend::None must succeed without running anything (no firewall to
        // configure → the port is already reachable).
        let rule = FirewallRule::snapclone_default();
        assert!(open_port(Backend::None, &rule).is_ok());
    }
}
