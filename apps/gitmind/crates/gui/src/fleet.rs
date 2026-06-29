//! A dedicated, GUI-managed **fleet user** instead of ssh-ing as your personal
//! account. Standard service-account practice: it isolates fleet access from your
//! login, the restricted-key hardening then scopes to a limited user, and the
//! default stops being "whoever ran `whoami`". The one cost is that *creating* the
//! user needs root once — `provision_fleet_user` does that over ssh+sudo during a
//! one-time setup (the actual useradd/adduser runs on-device; the command and the
//! config handling are unit-tested here).

use std::path::PathBuf;

/// Default fleet user when nothing is configured yet.
pub const FLEET_USER_DEFAULT: &str = "gitmind";

/// A username is interpolated into shell commands, so it must be strict: start with
/// a lowercase letter or `_`, then lowercase/digits/`_`/`-`, max 32 chars (the
/// portable POSIX-ish limit). Pure — unit-tested.
pub fn valid_username(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() || n.len() > 32 {
        return false;
    }
    let mut chars = n.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn config_path() -> Option<PathBuf> {
    config_file("fleet-user")
}

fn config_file(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/gitmind").join(name))
}

/// Remembered remote build repo paths, most-recent first (for the picker dropdown).
pub fn build_repos() -> Vec<String> {
    config_file("build-repo")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

/// The most-recently-used remote build repo (where the source lives on the build
/// host), or empty if unset.
pub fn build_repo() -> String {
    build_repos().into_iter().next().unwrap_or_default()
}

/// Remember a remote build repo path: move it to the front, dedupe, cap the history.
pub fn set_build_repo(path: &str) -> Result<(), String> {
    let p = path.trim().to_string();
    if p.is_empty() {
        return Ok(());
    }
    let mut hist = build_repos();
    hist.retain(|x| x != &p);
    hist.insert(0, p);
    hist.truncate(8);
    let f = config_file("build-repo").ok_or("no $HOME set")?;
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(&f, format!("{}\n", hist.join("\n"))).map_err(|e| format!("write {}: {e}", f.display()))
}

/// The configured fleet user (or the default). Managed by the GUI, not hardcoded.
pub fn fleet_user() -> String {
    config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| valid_username(s))
        .unwrap_or_else(|| FLEET_USER_DEFAULT.to_string())
}

/// Persist the fleet user the GUI should use.
pub fn set_fleet_user(name: &str) -> Result<(), String> {
    let n = name.trim();
    if !valid_username(n) {
        return Err(format!("'{n}' isn't a valid username"));
    }
    let p = config_path().ok_or("no $HOME set")?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(&p, format!("{n}\n")).map_err(|e| format!("write {}: {e}", p.display()))
}

/// Minimal base64 (standard alphabet) — used to ship the provisioning script to the
/// device without any shell-quoting hazard. Pure — unit-tested.
pub fn base64_encode(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { A[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// The provisioning script: create the fleet user (portable across `useradd` on
/// Fedora and busybox `adduser` on Alpine/postmarketOS), set up its `~/.ssh`, and
/// install the (already restriction-prefixed) key. The key is single-quoted so its
/// `from="…"` double-quotes are safe; the whole script is base64'd for transport.
/// Pure — unit-tested.
pub fn provision_user_script(user: &str, key_line: &str) -> String {
    let key = key_line.trim();
    format!(
        "set -e\n\
         if ! id -u '{user}' >/dev/null 2>&1; then \
           if command -v useradd >/dev/null 2>&1; then useradd -m -s /bin/sh '{user}'; \
           else adduser -D -s /bin/sh '{user}'; fi; \
         fi\n\
         H=$(getent passwd '{user}' 2>/dev/null | cut -d: -f6); [ -n \"$H\" ] || H=/home/{user}\n\
         mkdir -p \"$H/.ssh\"; chmod 700 \"$H/.ssh\"\n\
         printf '%s\\n' '{key}' >> \"$H/.ssh/authorized_keys\"\n\
         chmod 600 \"$H/.ssh/authorized_keys\"\n\
         chown -R '{user}':'{user}' \"$H/.ssh\"\n\
         echo gitmind-fleet-user-ready\n"
    )
}

/// The remote command run under the *bootstrap* (sudo-capable) account: decode the
/// base64 script and run it as root. The sudo password is fed on ssh stdin (`-S`);
/// the base64 is quote-safe. Pure — unit-tested.
pub fn provision_remote_command(b64_script: &str) -> String {
    format!("sudo -S sh -c \"$(printf %s '{b64_script}' | base64 -d)\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        assert!(valid_username("gitmind"));
        assert!(valid_username("_svc"));
        assert!(valid_username("fleet-1"));
        assert!(!valid_username(""));
        assert!(!valid_username("Git")); // uppercase
        assert!(!valid_username("1abc")); // leading digit
        assert!(!valid_username("a b")); // space
        assert!(!valid_username("a;rm -rf")); // injection chars
        assert!(!valid_username(&"x".repeat(33)));
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn provision_script_is_portable_and_quotes_the_key() {
        let key = "restrict,from=\"192.168.1.0/24\" ssh-ed25519 AAAAC3xyz gitmind-fleet";
        let s = provision_user_script("gitmind", key);
        assert!(s.contains("useradd -m") && s.contains("adduser -D"), "covers Fedora + Alpine");
        assert!(s.contains("'gitmind'"));
        // key sits inside single quotes so its from="..." double-quotes are safe
        assert!(s.contains(&format!("'{key}'")));
        assert!(s.contains("chmod 600") && s.contains("authorized_keys"));
        assert!(s.contains("gitmind-fleet-user-ready"));
    }

    #[test]
    fn remote_command_decodes_via_base64_and_sudo() {
        let b64 = base64_encode(b"echo hi\n");
        let cmd = provision_remote_command(&b64);
        assert!(cmd.starts_with("sudo -S sh -c"));
        assert!(cmd.contains(&b64) && cmd.contains("base64 -d"));
    }

    #[test]
    fn config_roundtrip_with_temp_home() {
        let tmp = std::env::temp_dir().join(format!("gm-fleet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);
        assert_eq!(fleet_user(), FLEET_USER_DEFAULT, "default when unset");
        set_fleet_user("fleetbot").unwrap();
        assert_eq!(fleet_user(), "fleetbot");
        assert!(set_fleet_user("Bad Name").is_err());
        // build repo persists as a most-recent-first history
        assert_eq!(build_repo(), "", "empty when unset");
        set_build_repo("/home/kesslerd/Code/monorepo").unwrap();
        set_build_repo("/home/kesslerd/other").unwrap();
        set_build_repo("/home/kesslerd/Code/monorepo").unwrap(); // re-pick → moves to front, no dup
        assert_eq!(build_repo(), "/home/kesslerd/Code/monorepo");
        assert_eq!(build_repos(), vec!["/home/kesslerd/Code/monorepo".to_string(), "/home/kesslerd/other".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
