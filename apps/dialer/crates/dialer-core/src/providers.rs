//! Carrier APN knowledge — sourced from **mobile-broadband-provider-info**,
//! the FOSS dataset NetworkManager and ModemManager themselves consume
//! (GNOME GitLab: mobile-broadband-provider-info, `serviceproviders.xml`).
//!
//! Three tiers, first hit wins:
//!
//! 1. **System database** — the distro-packaged XML at
//!    `/usr/share/mobile-broadband-provider-info/serviceproviders.xml`
//!    (postmarketOS/Alpine/Debian all ship it). Authoritative and updated by
//!    the distro.
//! 2. **A user-refreshed copy** at `$XDG_DATA_HOME/dialer/serviceproviders.xml`
//!    — "polling a FOSS API" is literally downloading the same XML from the
//!    project's raw URL; no bespoke HTTP client is needed for that, and the
//!    wizard shows the exact command.
//! 3. **The builtin snapshot** below — a hand-curated excerpt of the same
//!    dataset so the wizard is useful on a machine with neither file. It is a
//!    *snapshot*: correct at authoring time, and second-class to any XML.
//!
//! Matching: ModemManager reports the registered operator as an MCC+MNC
//! string ("310260"); providers list their `network-id`s the same way, so
//! suggestion is exact string match on the concatenation (MNC is 2 *or* 3
//! digits — never split the string to compare parts).

use std::path::PathBuf;

/// One configurable APN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Apn {
    pub carrier: String,
    /// ISO country code from the dataset ("us", "ca", …).
    pub country: String,
    /// MCC+MNC strings this APN serves ("310260").
    pub networks: Vec<String>,
    pub apn: String,
    pub username: String,
    pub password: String,
}

/// Where a loaded database came from — the wizard says which, because a user
/// staring at a wrong APN needs to know whether to update a package or blame
/// the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    SystemXml,
    UserXml,
    Builtin,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::SystemXml => "system provider database",
            Source::UserXml => "downloaded provider database",
            Source::Builtin => "built-in snapshot",
        }
    }
}

/// The distro path plus the user-refresh path.
pub fn candidate_paths() -> Vec<(PathBuf, Source)> {
    let mut v = vec![(
        PathBuf::from("/usr/share/mobile-broadband-provider-info/serviceproviders.xml"),
        Source::SystemXml,
    )];
    let user = crate::history::HistoryStore::default_path()
        .parent()
        .map(|d| d.join("serviceproviders.xml"));
    if let Some(p) = user {
        v.push((p, Source::UserXml));
    }
    v
}

/// The raw URL a refresh downloads (shown verbatim in the wizard).
pub const REFRESH_URL: &str =
    "https://gitlab.gnome.org/GNOME/mobile-broadband-provider-info/-/raw/main/serviceproviders.xml";

/// Load the best available database.
pub fn load() -> (Vec<Apn>, Source) {
    for (path, source) in candidate_paths() {
        if let Ok(xml) = std::fs::read_to_string(&path) {
            match parse_serviceproviders(&xml) {
                Ok(list) if !list.is_empty() => return (list, source),
                _ => {} // unreadable/empty file: fall through, don't fail setup
            }
        }
    }
    (builtin(), Source::Builtin)
}

/// Parse GNOME's `serviceproviders.xml` (format 2.0). Only `<gsm>` providers,
/// only internet-usage APNs (`<usage type="internet"/>` or no usage element —
/// MMS/WAP entries would mislead a data-setup wizard).
pub fn parse_serviceproviders(xml: &str) -> Result<Vec<Apn>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("providers xml: {e}"))?;
    let mut out = Vec::new();
    for country in doc.root_element().children().filter(|n| n.has_tag_name("country")) {
        let cc = country.attribute("code").unwrap_or("").to_string();
        for provider in country.children().filter(|n| n.has_tag_name("provider")) {
            let name = provider
                .children()
                .find(|n| n.has_tag_name("name"))
                .and_then(|n| n.text())
                .unwrap_or("")
                .trim()
                .to_string();
            for gsm in provider.children().filter(|n| n.has_tag_name("gsm")) {
                let networks: Vec<String> = gsm
                    .children()
                    .filter(|n| n.has_tag_name("network-id"))
                    .filter_map(|n| {
                        Some(format!("{}{}", n.attribute("mcc")?, n.attribute("mnc")?))
                    })
                    .collect();
                for apn in gsm.children().filter(|n| n.has_tag_name("apn")) {
                    let usage = apn
                        .children()
                        .find(|n| n.has_tag_name("usage"))
                        .and_then(|n| n.attribute("type"));
                    if !matches!(usage, None | Some("internet")) {
                        continue;
                    }
                    let value = apn.attribute("value").unwrap_or("").trim();
                    if value.is_empty() || name.is_empty() {
                        continue;
                    }
                    let text_of = |tag: &str| {
                        apn.children()
                            .find(|n| n.has_tag_name(tag))
                            .and_then(|n| n.text())
                            .unwrap_or("")
                            .trim()
                            .to_string()
                    };
                    out.push(Apn {
                        carrier: name.clone(),
                        country: cc.clone(),
                        networks: networks.clone(),
                        apn: value.to_string(),
                        username: text_of("username"),
                        password: text_of("password"),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// APNs whose network list contains the modem's reported MCC+MNC. Exact
/// match only — a wrong confident suggestion is worse than the full list.
pub fn suggest<'a>(all: &'a [Apn], mccmnc: &str) -> Vec<&'a Apn> {
    if mccmnc.is_empty() {
        return Vec::new();
    }
    all.iter().filter(|a| a.networks.iter().any(|n| n == mccmnc)).collect()
}

/// Case-insensitive carrier-name filter for the wizard's search box.
pub fn search<'a>(all: &'a [Apn], query: &str) -> Vec<&'a Apn> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return all.iter().collect();
    }
    all.iter()
        .filter(|a| a.carrier.to_lowercase().contains(&q) || a.apn.to_lowercase().contains(&q))
        .collect()
}

/// The curated snapshot (see module docs for its standing). Values excerpted
/// from mobile-broadband-provider-info; verify against the XML when present.
pub fn builtin() -> Vec<Apn> {
    fn apn(carrier: &str, country: &str, networks: &[&str], apn: &str, user: &str, pass: &str) -> Apn {
        Apn {
            carrier: carrier.into(),
            country: country.into(),
            networks: networks.iter().map(|s| s.to_string()).collect(),
            apn: apn.into(),
            username: user.into(),
            password: pass.into(),
        }
    }
    vec![
        // United States
        apn("AT&T", "us", &["310410", "310280", "310170", "310560", "310680"], "broadband", "", ""),
        apn("T-Mobile US", "us", &["310260", "310160", "310200", "310210", "310220", "310230", "310240", "310250", "310660", "310800"], "fast.t-mobile.com", "", ""),
        apn("Verizon", "us", &["311480", "310004", "310012"], "vzwinternet", "", ""),
        // Canada
        apn("Rogers", "ca", &["302720"], "ltemobile.apn", "", ""),
        apn("Bell", "ca", &["302610"], "pda.bell.ca", "", ""),
        apn("Telus", "ca", &["302220"], "sp.telus.com", "", ""),
        apn("Fido", "ca", &["302370"], "ltemobile.apn", "", ""),
        // United Kingdom
        apn("O2 UK", "gb", &["23410", "23411"], "mobile.o2.co.uk", "o2web", "password"),
        apn("EE", "gb", &["23430", "23431", "23432"], "everywhere", "eesecure", "secure"),
        apn("Vodafone UK", "gb", &["23415"], "internet", "web", "web"),
        // Germany
        apn("Telekom.de", "de", &["26201"], "internet.telekom", "telekom", "tm"),
        apn("Vodafone DE", "de", &["26202"], "web.vodafone.de", "", ""),
        apn("O2 DE", "de", &["26203", "26207"], "internet", "", ""),
        // France
        apn("Orange", "fr", &["20801", "20802"], "orange.fr", "orange", "orange"),
        apn("SFR", "fr", &["20810"], "sl2sfr", "", ""),
        apn("Free Mobile", "fr", &["20815"], "free", "", ""),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<serviceproviders format="2.0">
  <country code="us">
    <provider>
      <name>T-Mobile</name>
      <gsm>
        <network-id mcc="310" mnc="260"/>
        <network-id mcc="310" mnc="160"/>
        <apn value="fast.t-mobile.com">
          <usage type="internet"/>
          <name>Internet</name>
        </apn>
        <apn value="mms.t-mobile.com">
          <usage type="mms"/>
        </apn>
      </gsm>
    </provider>
  </country>
  <country code="ca">
    <provider>
      <name>Rogers</name>
      <gsm>
        <network-id mcc="302" mnc="720"/>
        <apn value="ltemobile.apn">
          <username>wapuser1</username>
          <password>wap</password>
        </apn>
      </gsm>
    </provider>
    <provider>
      <name>Cable Co</name>
      <cdma><username>x</username></cdma>
    </provider>
  </country>
</serviceproviders>"#;

    #[test]
    fn parses_gsm_internet_apns_and_skips_mms_and_cdma() {
        let list = parse_serviceproviders(SAMPLE).unwrap();
        assert_eq!(list.len(), 2, "one internet APN each; MMS and CDMA skipped");
        let tmo = &list[0];
        assert_eq!(tmo.carrier, "T-Mobile");
        assert_eq!(tmo.apn, "fast.t-mobile.com");
        assert_eq!(tmo.networks, ["310260", "310160"]);
        let rogers = &list[1];
        assert_eq!((rogers.username.as_str(), rogers.password.as_str()), ("wapuser1", "wap"));
        assert_eq!(rogers.country, "ca");
    }

    #[test]
    fn suggestion_is_exact_mccmnc_match() {
        let list = parse_serviceproviders(SAMPLE).unwrap();
        let hits = suggest(&list, "310260");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].carrier, "T-Mobile");
        // "31026" or "3102" must NOT match — MNC length is meaningful.
        assert!(suggest(&list, "31026").is_empty());
        assert!(suggest(&list, "").is_empty());
    }

    #[test]
    fn search_matches_carrier_or_apn_case_insensitively() {
        let all = builtin();
        assert!(search(&all, "rogers").iter().any(|a| a.carrier == "Rogers"));
        assert!(search(&all, "FAST.T-MOBILE").iter().any(|a| a.carrier == "T-Mobile US"));
        assert_eq!(search(&all, "").len(), all.len());
    }

    #[test]
    fn builtin_networks_are_all_plausible_mccmnc_strings() {
        for a in builtin() {
            assert!(!a.networks.is_empty(), "{} has no networks", a.carrier);
            for n in &a.networks {
                assert!(
                    (5..=6).contains(&n.len()) && n.chars().all(|c| c.is_ascii_digit()),
                    "{}: bad network id {n}",
                    a.carrier
                );
            }
        }
    }
}
