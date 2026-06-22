//! v0.1.703: ssh voice control — "connect to prod" / "ssh into my server" →
//! an `ssh <target>` command, resolving a spoken alias against the user's
//! ~/.ssh/config Host entries. Clean-room from the public ssh_config format;
//! pure + testable (config parsing + alias matching are the testable cores;
//! reading the file + running ssh are the runtime layer).

/// A connect request parsed from speech.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshRequest {
    /// the spoken alias / host phrase (e.g. "prod", "my server")
    pub alias: String,
    /// optional command to run on the host ("run uptime on prod")
    pub run: Option<String>,
}

/// Parse a spoken phrase into an SSH connect request. None if it isn't one.
/// Forms: "ssh to <h>", "ssh into <h>", "connect to <h>", "log into <h>",
/// "run <cmd> on <h>", "ssh <h>".
pub fn parse(phrase: &str) -> Option<SshRequest> {
    let t = phrase.trim().to_lowercase();
    let t = t.as_str();

    // "run <cmd> on <host>"
    if let Some(rest) = t.strip_prefix("run ") {
        if let Some(idx) = rest.rfind(" on ") {
            let cmd = rest[..idx].trim();
            let host = rest[idx + 4..].trim();
            if !cmd.is_empty() && !host.is_empty() {
                return Some(SshRequest { alias: host.to_string(), run: Some(cmd.to_string()) });
            }
        }
        return None;
    }

    // connection openers → strip, the remainder is the alias
    let openers = [
        "ssh into the ", "ssh into ", "ssh to the ", "ssh to ", "ssh ",
        "connect to the ", "connect to ", "log into the ", "log into ",
        "log in to ", "shell into ", "remote into ",
    ];
    for op in openers {
        if let Some(rest) = t.strip_prefix(op) {
            let alias = rest.trim().trim_end_matches(" server").trim_end_matches(" host").trim();
            if !alias.is_empty() {
                return Some(SshRequest { alias: alias.to_string(), run: None });
            }
        }
    }
    None
}

/// A Host entry from ~/.ssh/config (alias → the connect target it implies).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    /// the Host pattern token (e.g. "prod", "web-1"); wildcards are skipped.
    pub alias: String,
    /// HostName if given, else the alias itself.
    pub hostname: String,
    pub user: Option<String>,
}

/// Parse the Host entries out of an ssh_config body. Only concrete (no `*`/`?`)
/// Host tokens are kept — those are the ones a user can name by voice.
pub fn parse_ssh_config(body: &str) -> Vec<HostEntry> {
    let mut entries: Vec<HostEntry> = Vec::new();
    let mut cur_aliases: Vec<String> = Vec::new();
    let mut hostname: Option<String> = None;
    let mut user: Option<String> = None;

    let flush = |entries: &mut Vec<HostEntry>, aliases: &[String],
                 hostname: &Option<String>, user: &Option<String>| {
        for a in aliases {
            entries.push(HostEntry {
                alias: a.clone(),
                hostname: hostname.clone().unwrap_or_else(|| a.clone()),
                user: user.clone(),
            });
        }
    };

    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (key, val) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim().to_string()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                // flush the previous block
                flush(&mut entries, &cur_aliases, &hostname, &user);
                cur_aliases = val.split_whitespace()
                    .filter(|t| !t.contains('*') && !t.contains('?'))
                    .map(|s| s.to_string()).collect();
                hostname = None;
                user = None;
            }
            "hostname" => hostname = Some(val),
            "user" => user = Some(val),
            _ => {}
        }
    }
    flush(&mut entries, &cur_aliases, &hostname, &user);
    entries
}

/// Resolve a spoken alias to a HostEntry by best match: exact alias, then
/// case-insensitive, then a normalized match (spaces↔hyphens), then prefix.
pub fn resolve(alias: &str, hosts: &[HostEntry]) -> Option<HostEntry> {
    let a = alias.trim().to_lowercase();
    let norm = |s: &str| s.to_lowercase().replace([' ', '_'], "-");
    let an = norm(&a);
    // exact (case-insensitive)
    if let Some(h) = hosts.iter().find(|h| h.alias.to_lowercase() == a) {
        return Some(h.clone());
    }
    // normalized (spoken "web one" vs config "web-1" won't match, but
    // "my-server" vs "my server" will)
    if let Some(h) = hosts.iter().find(|h| norm(&h.alias) == an) {
        return Some(h.clone());
    }
    // prefix
    hosts.iter().find(|h| norm(&h.alias).starts_with(&an)).cloned()
}

/// Build the `ssh` command line for a resolved host + optional remote command.
pub fn to_command(h: &HostEntry, run: &Option<String>) -> String {
    let target = match &h.user {
        Some(u) => format!("{}@{}", u, h.hostname),
        None => h.hostname.clone(),
    };
    match run {
        Some(cmd) => format!("ssh {} {}", target, shell_quote(cmd)),
        None => format!("ssh {}", target),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = "\
# my hosts
Host prod
    HostName prod.example.com
    User deploy

Host web-1 web-2
    HostName 10.0.0.5

Host *
    ServerAliveInterval 60
";

    #[test]
    fn parse_connect_phrases() {
        assert_eq!(parse("connect to prod"), Some(SshRequest { alias: "prod".into(), run: None }));
        assert_eq!(parse("ssh into the web server"),
            Some(SshRequest { alias: "web".into(), run: None }));
        assert_eq!(parse("ssh prod"), Some(SshRequest { alias: "prod".into(), run: None }));
        assert_eq!(parse("log into staging"),
            Some(SshRequest { alias: "staging".into(), run: None }));
    }

    #[test]
    fn parse_run_on_host() {
        assert_eq!(parse("run uptime on prod"),
            Some(SshRequest { alias: "prod".into(), run: Some("uptime".into()) }));
        assert_eq!(parse("run docker ps on web-1"),
            Some(SshRequest { alias: "web-1".into(), run: Some("docker ps".into()) }));
    }

    #[test]
    fn non_ssh_is_none() {
        assert_eq!(parse("open google"), None);
        assert_eq!(parse("what's the weather"), None);
    }

    #[test]
    fn config_parses_concrete_hosts_only() {
        let hosts = parse_ssh_config(CFG);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert!(aliases.contains(&"prod"));
        assert!(aliases.contains(&"web-1"));
        assert!(aliases.contains(&"web-2"));
        assert!(!aliases.iter().any(|a| a.contains('*')), "wildcard Host skipped");
    }

    #[test]
    fn config_carries_hostname_and_user() {
        let hosts = parse_ssh_config(CFG);
        let prod = hosts.iter().find(|h| h.alias == "prod").unwrap();
        assert_eq!(prod.hostname, "prod.example.com");
        assert_eq!(prod.user.as_deref(), Some("deploy"));
        // multi-alias block shares the HostName, no user
        let w1 = hosts.iter().find(|h| h.alias == "web-1").unwrap();
        assert_eq!(w1.hostname, "10.0.0.5");
        assert!(w1.user.is_none());
    }

    #[test]
    fn resolve_and_command() {
        let hosts = parse_ssh_config(CFG);
        let prod = resolve("prod", &hosts).unwrap();
        assert_eq!(to_command(&prod, &None), "ssh deploy@prod.example.com");
        // run-on
        assert_eq!(to_command(&prod, &Some("uptime".into())),
                   "ssh deploy@prod.example.com 'uptime'");
        // no-user host
        let w = resolve("web-1", &hosts).unwrap();
        assert_eq!(to_command(&w, &None), "ssh 10.0.0.5");
    }

    #[test]
    fn resolve_normalized_and_prefix() {
        let hosts = vec![
            HostEntry { alias: "my-server".into(), hostname: "ms.local".into(), user: None },
        ];
        // spoken "my server" → "my-server"
        assert!(resolve("my server", &hosts).is_some());
        // prefix
        assert!(resolve("my", &hosts).is_some());
        assert!(resolve("nonexistent", &hosts).is_none());
    }
}
