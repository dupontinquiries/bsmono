// src/sandbox.rs
// Security sandboxing: restrict what the browser component can access.
// The PTY terminal has full access (it's a shell), but the web rendering
// pipeline should be sandboxed to prevent:
// - Fetching file:// URLs that access sensitive files
// - Form submissions to internal/private IPs
// - JavaScript-equivalent actions that could exfiltrate data
// - Loading content from known malicious domains


/// Check if a URL is safe to fetch
pub fn is_safe_url(url: &str) -> Result<(), &'static str> {
    let lower = url.to_ascii_lowercase();

    // Block file:// access to sensitive paths
    if lower.starts_with("file://") {
        let path = &url[7..];
        if is_sensitive_path(path) {
            return Err("Access to sensitive file blocked");
        }
    }

    // Block data: URLs that could be used for exfiltration
    if lower.starts_with("data:") && lower.len() > 1000 {
        return Err("Large data: URL blocked");
    }

    // Block javascript: URLs
    if lower.starts_with("javascript:") {
        return Err("javascript: URL blocked");
    }

    // Block known malicious/tracking domains
    if let Some(domain) = extract_domain(&lower) {
        if is_blocked_domain(&domain) {
            return Err("Blocked domain");
        }
    }

    Ok(())
}

/// Check if a URL is safe for form submission
pub fn is_safe_form_target(url: &str) -> Result<(), &'static str> {
    is_safe_url(url)?;

    // Block form submission to private/internal IPs.
    // v0.1.467: lowercase before extracting/checking the domain.
    // Domains are case-INSENSITIVE, but is_private_ip compares
    // against lowercase literals ("localhost", …). The prior code
    // passed the ORIGINAL-case url, so `http://LOCALHOST/x` (or any
    // mixed case) bypassed the private-IP block entirely — an
    // SSRF-adjacent gap the sandbox is meant to close. (is_safe_url
    // above already lowercases for ITS checks; this second check
    // needs the same treatment.)
    let lower = url.to_ascii_lowercase();
    if let Some(domain) = extract_domain(&lower) {
        if is_private_ip(&domain) {
            return Err("Form submission to private IP blocked");
        }
    }

    Ok(())
}

/// Check if a file path points to sensitive system files
fn is_sensitive_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    // Block access to common sensitive locations
    p.starts_with("/etc/") ||
    p.starts_with("/root/") ||
    p.contains("/.ssh/") ||
    p.contains("/.gnupg/") ||
    p.contains("/.config/") && !p.contains("web2qml") ||
    p.contains("/.local/share/keyrings") ||
    p.contains("/shadow") ||
    p.contains("/passwd") && !p.contains("password") ||
    p.starts_with("/proc/") ||
    p.starts_with("/sys/") ||
    p.starts_with("/dev/") ||
    p.contains("/.bash_history") ||
    p.contains("/.zsh_history") ||
    p.contains("id_rsa") ||
    p.contains("id_ed25519") ||
    p.contains("known_hosts") ||
    p.contains("authorized_keys") ||
    p.contains("wallet.dat") ||
    p.contains(".env") ||
    p.contains("credentials") ||
    p.contains("token") && p.contains(".json")
}

/// Check if a domain resolves to a private/internal IP
fn is_private_ip(domain: &str) -> bool {
    // Check common private IP patterns
    if domain == "localhost" || domain == "127.0.0.1" || domain == "::1" { return true; }
    if domain.starts_with("192.168.") || domain.starts_with("10.") { return true; }
    if domain.starts_with("172.") {
        if let Some(second) = domain.split('.').nth(1) {
            if let Ok(n) = second.parse::<u8>() {
                if (16..=31).contains(&n) { return true; }
            }
        }
    }
    if domain.starts_with("169.254.") { return true; } // link-local
    if domain == "0.0.0.0" { return true; }
    false
}

fn is_blocked_domain(domain: &str) -> bool {
    const BLOCKED: &[&str] = &[
        // Cryptocurrency miners
        "coinhive.com", "coin-hive.com", "cryptoloot.pro",
        // Known phishing infrastructure
        "evil.com",
        // Extremely aggressive trackers
        "pixel.facebook.com",
    ];
    BLOCKED.iter().any(|b| domain.ends_with(b))
}

fn extract_domain(url: &str) -> Option<String> {
    if let Some(start) = url.find("://") {
        let rest = &url[start + 3..];
        let end = rest.find('/').unwrap_or(rest.len());
        let host = &rest[..end];
        // Strip port
        let host = if let Some(p) = host.rfind(':') {
            if host[p+1..].chars().all(|c| c.is_ascii_digit()) { &host[..p] } else { host }
        } else { host };
        Some(host.to_string())
    } else { None }
}

/// Sanitize HTML text content to prevent QML injection
/// This runs AFTER write_esc but catches edge cases
pub fn sanitize_text_for_qml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\0'..='\x08' | '\x0b' | '\x0c' | '\x0e'..='\x1f' | '\x7f' => {} // strip C0 controls
            _ => out.push(c),
        }
    }
    out
}

/// Rate limiter for HTTP requests to prevent abuse
pub struct RateLimiter {
    requests: Vec<std::time::Instant>,
    max_per_minute: usize,
}

impl RateLimiter {
    pub fn new(max_per_minute: usize) -> Self {
        RateLimiter { requests: Vec::new(), max_per_minute }
    }

    pub fn check(&mut self) -> bool {
        let now = std::time::Instant::now();
        let cutoff = now - std::time::Duration::from_secs(60);
        self.requests.retain(|t| *t > cutoff);
        if self.requests.len() >= self.max_per_minute { return false; }
        self.requests.push(now);
        true
    }
}

/// v0.1.619: shell-quote a path for safe inclusion in a `cd <path>`
/// command. Pure + tested, so the Rust and QML implementations can't
/// silently diverge (they did: the old main.rs shell_quote wrapped the
/// WHOLE string when a special char was present, turning "~/my dir" into
/// '~/my dir' — a LITERAL tilde the shell won't expand, breaking `cd`
/// into home-relative dirs with spaces. The QML shellQuote already
/// peeled the tilde prefix correctly; this brings Rust in line.)
///
/// Bare-safe set: ASCII alphanumeric + / . _ - ~ (matches QML). If the
/// string is fully bare-safe it's returned unchanged. Otherwise the
/// `~`, `~/`, or `~user/` prefix (if any) is peeled and left UNquoted so
/// the shell still expands it, and the remainder is single-quoted with
/// internal single-quotes escaped as '\''.
pub fn shell_quote(p: &str) -> String {
    if p.is_empty() {
        return "''".to_string();
    }
    let bare_safe = |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~')
    };
    if p.chars().all(bare_safe) {
        return p.to_string();
    }
    // Needs quoting. Peel a leading tilde prefix so it still expands.
    let (prefix, rest): (&str, &str) = if let Some(after) = p.strip_prefix('~') {
        if after.is_empty() {
            // bare "~" — but we only get here if NOT all-bare-safe, which
            // can't happen for a lone "~"; defensive.
            return p.to_string();
        } else if let Some(r) = after.strip_prefix('/') {
            ("~/", r) // "~/..." → prefix "~/", quote the rest
        } else if let Some(slash) = p.find('/') {
            // "~user/..." → leave "~user/" bare, quote the rest
            (&p[..slash + 1], &p[slash + 1..])
        } else {
            // "~user" with no slash — pass through bare (rare, valid)
            return p.to_string();
        }
    } else {
        ("", p)
    };
    if rest.is_empty() {
        return prefix.to_string();
    }
    let escaped = rest.replace('\'', "'\\''");
    format!("{}'{}'", prefix, escaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_bare_paths_unchanged() {
        assert_eq!(shell_quote("/tmp/x"), "/tmp/x");
        assert_eq!(shell_quote("~/projects"), "~/projects");
        assert_eq!(shell_quote("a_b-c.d"), "a_b-c.d");
        assert_eq!(shell_quote("~"), "~");
    }

    #[test]
    fn shell_quote_empty_is_empty_quotes() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_spaces_get_quoted() {
        assert_eq!(shell_quote("/tmp/my dir"), "'/tmp/my dir'");
    }

    #[test]
    fn shell_quote_tilde_prefix_preserved_for_expansion() {
        // THE divergence the old Rust impl got wrong: it produced
        // '~/my dir' (literal ~, no expansion). Correct is ~/'my dir'.
        assert_eq!(shell_quote("~/my dir"), "~/'my dir'");
    }

    #[test]
    fn shell_quote_tilde_user_prefix_preserved() {
        assert_eq!(shell_quote("~user/a b"), "~user/'a b'");
    }

    #[test]
    fn shell_quote_internal_single_quote_escaped() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        // and combined with a tilde prefix
        assert_eq!(shell_quote("~/x'y"), "~/'x'\\''y'");
    }

    #[test]
    fn shell_quote_no_literal_tilde_inside_quotes() {
        // a quoted result must never contain "'~" (literal tilde wrapped)
        for p in ["~/my dir", "~user/a b", "~/x'y"] {
            let q = shell_quote(p);
            assert!(!q.starts_with("'~"), "{:?} wrapped the tilde: {:?}", p, q);
        }
    }

    #[test]
    fn blocks_sensitive_files() {
        assert!(is_safe_url("file:///etc/passwd").is_err());
        assert!(is_safe_url("file:///root/secret").is_err());
        assert!(is_safe_url("file:///home/u/.ssh/id_rsa").is_err());
        assert!(is_safe_url("file:///home/u/.gnupg/secring").is_err());
        assert!(is_safe_url("file:///home/u/wallet.dat").is_err());
        assert!(is_safe_url("file:///app/.env").is_err());
    }

    #[test]
    fn allows_own_config() {
        // web2qml's own config dir is exempt from the .config block.
        assert!(is_safe_url("file:///home/u/.config/web2qml/settings.conf").is_ok());
    }

    #[test]
    fn allows_normal_urls() {
        assert!(is_safe_url("https://example.com/page").is_ok());
        assert!(is_safe_url("http://news.site.org/article/123").is_ok());
    }

    #[test]
    fn blocks_javascript_and_large_data() {
        assert!(is_safe_url("javascript:alert(1)").is_err());
        let big = format!("data:text/html,{}", "x".repeat(1001));
        assert!(is_safe_url(&big).is_err());
        // Small data: URLs are allowed.
        assert!(is_safe_url("data:text/plain,hello").is_ok());
    }

    #[test]
    fn blocks_blocked_domains() {
        assert!(is_safe_url("https://coinhive.com/miner.js").is_err());
        assert!(is_safe_url("https://sub.evil.com/x").is_err());
    }

    #[test]
    fn form_blocks_private_ips() {
        assert!(is_safe_form_target("http://192.168.1.1/submit").is_err());
        assert!(is_safe_form_target("http://10.0.0.5/x").is_err());
        assert!(is_safe_form_target("http://localhost/x").is_err());
        assert!(is_safe_form_target("http://127.0.0.1/x").is_err());
        assert!(is_safe_form_target("http://169.254.1.1/x").is_err());
    }

    #[test]
    fn form_172_private_range_boundary() {
        // Private: 172.16.0.0 – 172.31.255.255.
        assert!(is_safe_form_target("http://172.16.0.1/x").is_err());
        assert!(is_safe_form_target("http://172.31.255.1/x").is_err());
        // Public just outside the range.
        assert!(is_safe_form_target("http://172.15.0.1/x").is_ok());
        assert!(is_safe_form_target("http://172.32.0.1/x").is_ok());
    }

    #[test]
    fn form_private_ip_case_insensitive() {
        // v0.1.467 regression guard: domains are case-insensitive,
        // so mixed/upper case must NOT bypass the private-IP block.
        assert!(is_safe_form_target("http://LOCALHOST/x").is_err());
        assert!(is_safe_form_target("http://LocalHost/x").is_err());
    }

    #[test]
    fn uppercase_scheme_ok() {
        assert!(is_safe_url("HTTPS://example.com/").is_ok());
    }

    #[test]
    fn extract_domain_strips_port() {
        assert_eq!(extract_domain("https://example.com:8443/x"),
            Some("example.com".to_string()));
        // Non-numeric "port" is not stripped (kept as host).
        assert_eq!(extract_domain("https://example.com/x"),
            Some("example.com".to_string()));
    }

    #[test]
    fn rate_limiter_enforces_limit() {
        let mut rl = RateLimiter::new(3);
        assert!(rl.check());
        assert!(rl.check());
        assert!(rl.check());
        assert!(!rl.check()); // over limit
    }

    #[test]
    fn sanitize_strips_c0_controls() {
        assert_eq!(sanitize_text_for_qml("a\x00b\x07c\x1fd"), "abcd");
        // Printable + common whitespace preserved.
        assert_eq!(sanitize_text_for_qml("hi\tthere\n"), "hi\tthere\n");
    }
}
