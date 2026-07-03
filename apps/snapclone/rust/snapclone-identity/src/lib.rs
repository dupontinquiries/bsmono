//! sc-ID generation and peer identity.
//!
//! The sc-ID is a stable 32-bit random identifier formatted as "sc-XXXXXXXX"
//! (8 hex digits). It is generated once on first launch and persisted.
//! Ported from AppController::initialize() identity setup.

use rand::Rng;

/// Generate a fresh sc-ID ("sc-XXXXXXXX").
/// Equivalent to the C++ QRandomGenerator / UUID truncation in AppController.
pub fn generate_sc_id() -> String {
    let random: u32 = rand::thread_rng().gen();
    format!("sc-{:08x}", random)
}

/// Validate that a string looks like a well-formed sc-ID.
pub fn is_valid_sc_id(s: &str) -> bool {
    s.len() == 11
        && s.starts_with("sc-")
        && s[3..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Extract the bare sc-ID from a display string like "user@ip (sc-32a51661)".
/// Returns `Some("sc-32a51661")` or `None` if not found.
pub fn extract_sc_id(display: &str) -> Option<String> {
    let start = display.find("(sc-")?;
    let end   = display[start..].find(')')? + start;
    let id    = &display[start + 1..end]; // strip parens
    if is_valid_sc_id(id) { Some(id.to_owned()) } else { None }
}

/// Strip the ip/sc-id suffix from a realId, returning just the username.
/// "kesslerd@192.168.1.5 (sc-32a51661)" → "kesslerd"
pub fn extract_username(real_id: &str) -> String {
    let base = real_id.find(" (").map(|i| &real_id[..i]).unwrap_or(real_id);
    let user = base.find('@').map(|i| &base[..i]).unwrap_or(base);
    if user.is_empty() { real_id.to_owned() } else { user.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sc_id_format() {
        let id = generate_sc_id();
        assert!(is_valid_sc_id(&id), "Bad sc-ID: {}", id);
    }

    #[test]
    fn extract_from_display() {
        let r = extract_sc_id("kesslerd@192.168.1.5 (sc-32a51661)");
        assert_eq!(r.as_deref(), Some("sc-32a51661"));
    }

    #[test]
    fn extract_username_ok() {
        assert_eq!(extract_username("kesslerd@192.168.1.5 (sc-32a51661)"), "kesslerd");
        assert_eq!(extract_username("sc-32a51661"), "sc-32a51661"); // no @ → use full
    }

    #[test]
    fn invalid_sc_id() {
        assert!(!is_valid_sc_id("sc-xyz"));        // non-hex
        assert!(!is_valid_sc_id("sc-1234567890")); // too long
        assert!(!is_valid_sc_id("ab-12345678"));   // wrong prefix
    }
}
