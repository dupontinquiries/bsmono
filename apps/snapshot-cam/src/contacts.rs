//! contacts — snapclone's contact book, read for the send picker.
//!
//! `contacts.conf` under snapclone's config dir: one INI section per
//! contact, keyed by sc-ID, with a `nickname`. Read only, and read
//! directly rather than through the snapclone crate, which this app does
//! not link. A contact that is not in snapclone cannot be sent to — the
//! outbox request names an sc-ID snapclone must already know.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub sc_id: String,
    pub nickname: String,
}

impl Contact {
    pub fn label(&self) -> &str {
        if self.nickname.trim().is_empty() { &self.sc_id } else { &self.nickname }
    }
}

pub fn parse(text: &str) -> Vec<Contact> {
    let ini = config_kit::Ini::parse(text);
    let mut out: Vec<(u64, Contact)> = ini.sections.keys()
        .filter(|s| s.starts_with("sc-"))
        .map(|s| {
            let added: u64 = ini.get(s, "added_ms").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
            (added, Contact { sc_id: s.clone(), nickname: ini.get(s, "nickname").unwrap_or("").trim().to_string() })
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sc_id.cmp(&b.1.sc_id)));
    out.into_iter().map(|(_, c)| c).collect()
}

/// The contacts on this device, oldest first as snapclone lists them.
pub fn load() -> Vec<Contact> {
    config_kit::load("snapclone", "contacts.conf").map(|t| parse(&t)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_book_parses_with_labels_and_skips_non_contacts() {
        let text = "# snapclone contacts\n\n[sc-2dd464a4]\nnickname = Bee\nport = 41235\nadded_ms = 5\n\n[sc-11111111]\nnickname =\nadded_ms = 1\n\n[settings]\nnickname = x\n";
        let cs = parse(text);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].label(), "sc-11111111");
        assert_eq!(cs[1].label(), "Bee");
    }
}
