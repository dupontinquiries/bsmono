//! trust -- the devices this one has agreed to sync with. A plain text file, one device a line, readable and
//! editable by hand; removing a line revokes the device. Sync talks only to a device that is on this list.

use crate::identity::{clean_name, fingerprint, hex, unhex};
use crate::pairing::Offer;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub ed_pub: [u8; 32],
    pub x_pub: [u8; 32],
    /// The last `host:port` seen for it.
    pub addr: Option<String>,
    /// Unix seconds when it was trusted.
    pub added: u64,
}

impl Peer {
    pub fn fingerprint(&self) -> String { fingerprint(&self.ed_pub) }
    pub fn id(&self) -> String { hex(&self.ed_pub[..8]) }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TrustStore {
    peers: Vec<Peer>,
}

impl TrustStore {
    pub fn peers(&self) -> &[Peer] { &self.peers }

    pub fn is_trusted(&self, ed_pub: &[u8; 32]) -> bool { self.peers.iter().any(|p| &p.ed_pub == ed_pub) }

    pub fn find(&self, ed_pub: &[u8; 32]) -> Option<&Peer> { self.peers.iter().find(|p| &p.ed_pub == ed_pub) }

    /// Trust the device an offer describes (after its owner said yes). Trusting a device again updates its name,
    /// keys-for-agreement and address instead of adding a second line. Returns whether it was new.
    pub fn trust(&mut self, offer: &Offer, now_s: u64) -> bool {
        let addr = offer.addrs.first().cloned();
        if let Some(p) = self.peers.iter_mut().find(|p| p.ed_pub == offer.ed_pub) {
            p.name = offer.name.clone();
            p.x_pub = offer.x_pub;
            if addr.is_some() { p.addr = addr; }
            return false;
        }
        self.peers.push(Peer { name: offer.name.clone(), ed_pub: offer.ed_pub, x_pub: offer.x_pub, addr, added: now_s });
        true
    }

    /// Stop trusting `id` (the 16 hex digits, or any prefix of at least 4). Returns whether one was removed.
    pub fn revoke(&mut self, id: &str) -> bool {
        let id = id.trim().to_ascii_lowercase();
        if id.len() < 4 { return false; }
        let before = self.peers.len();
        self.peers.retain(|p| !p.id().starts_with(&id));
        self.peers.len() != before
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("jotter trust 1\n# one device a line: ed_pub x_pub added addr name. Delete a line to revoke it.\n");
        for p in &self.peers {
            s.push_str(&format!("{} {} {} {} {}\n", hex(&p.ed_pub), hex(&p.x_pub), p.added, p.addr.clone().unwrap_or_else(|| "-".into()), p.name));
        }
        s
    }

    /// Lines that do not parse are skipped (a half-edited file does not lose the rest).
    pub fn parse(text: &str) -> TrustStore {
        let mut peers = Vec::new();
        for line in text.lines() {
            if line.starts_with('#') || line.starts_with("jotter trust") || line.trim().is_empty() { continue; }
            let mut it = line.splitn(5, ' ');
            let (Some(e), Some(x), Some(t), Some(a), Some(n)) = (it.next(), it.next(), it.next(), it.next(), it.next()) else { continue };
            let (Some(ed_pub), Some(x_pub), Ok(added)) = (unhex::<32>(e), unhex::<32>(x), t.parse::<u64>()) else { continue };
            peers.push(Peer { name: clean_name(n), ed_pub, x_pub, addr: (a != "-").then(|| a.to_string()), added });
        }
        TrustStore { peers }
    }

    pub fn load(path: &std::path::Path) -> TrustStore {
        std::fs::read_to_string(path).map(|t| TrustStore::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() { std::fs::create_dir_all(d)?; }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }

    /// `$XDG_DATA_HOME/jotter/trusted`, else `~/.local/share/jotter/trusted`.
    pub fn default_path() -> Option<std::path::PathBuf> {
        crate::identity::Identity::default_path().map(|p| p.with_file_name("trusted"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn offer(name: &str, addr: &str) -> (Identity, Offer) {
        let id = Identity::generate(name);
        let o = Offer::new(&id, vec![addr.into()], 10);
        (id, o)
    }

    #[test]
    fn trusting_is_explicit_once_per_device_and_revocable() {
        let (_, phone) = offer("Phone", "192.168.2.231:7471");
        let (_, laptop) = offer("Laptop", "192.168.2.50:7471");
        let mut t = TrustStore::default();
        assert!(!t.is_trusted(&phone.ed_pub), "nothing is trusted by default");
        assert!(t.trust(&phone, 100) && t.trust(&laptop, 101));
        assert!(t.is_trusted(&phone.ed_pub) && t.is_trusted(&laptop.ed_pub));
        let again = Offer { name: "Phone (new name)".into(), addrs: vec!["10.0.0.9:7471".into()], ..phone.clone() };
        assert!(!t.trust(&again, 200), "the same device again is an update, not a second entry");
        assert_eq!(t.peers().len(), 2);
        assert_eq!(t.find(&phone.ed_pub).unwrap().addr.as_deref(), Some("10.0.0.9:7471"));
        assert_eq!(t.find(&phone.ed_pub).unwrap().added, 100, "the date it was first trusted stays");
        let id = t.find(&laptop.ed_pub).unwrap().id();
        assert!(!t.revoke("ab"), "too short a prefix revokes nothing");
        assert!(t.revoke(&id[..6]));
        assert!(!t.is_trusted(&laptop.ed_pub) && t.is_trusted(&phone.ed_pub));
        assert!(!t.revoke(&id), "already gone");
    }

    #[test]
    fn the_list_round_trips_through_its_file_and_a_damaged_line_costs_only_itself() {
        let (_, a) = offer("Mara's phone", "192.168.2.231:7471");
        let (_, b) = offer("Tablet", "x");
        let mut t = TrustStore::default();
        t.trust(&a, 5);
        t.trust(&Offer { addrs: vec![], ..b }, 6);
        assert_eq!(TrustStore::parse(&t.to_text()), t);
        let damaged = t.to_text().replace(&hex(&a.x_pub), "zz");
        let got = TrustStore::parse(&damaged);
        assert_eq!(got.peers().len(), 1, "the damaged line is skipped, the other kept");
        assert_eq!(got.peers()[0].name, "Tablet");
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("deep").join("trusted");
        t.save(&p).unwrap();
        assert_eq!(TrustStore::load(&p), t);
        assert_eq!(TrustStore::load(&d.path().join("missing")), TrustStore::default());
    }
}
