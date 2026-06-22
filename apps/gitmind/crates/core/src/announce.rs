// gitmind-core/src/announce.rs
//
// Project broadcast — the data model for "advertise a project and its
// description so it can be found and fetched", over an eventual DHT or via
// hosting providers. It mirrors the RemoteTarget seam (remote.rs): this
// file fixes the PAYLOAD; the transports (Kademlia DHT, hosting-provider
// HTTP APIs) are impls added later.
//
// A ProjectCard is content-addressed: its id is the BLAKE3 of its bytes,
// so the same announcement has the same id everywhere and can't be
// silently altered. Two keys are published:
//   - name_key  = hash(name)  → discover a project by name
//   - id        = hash(card)  → fetch an exact, verified announcement
// The `providers` list tells a peer WHERE to fetch the actual chunks/refs
// (an ssh:// host, an rclone remote, or a DHT provider record), reusing the
// RemoteTarget machinery. Signing (ed25519) is added with the encryption
// phase so a card's authenticity can be verified; the unsigned card is the
// payload that gets signed.

use crate::oid::Oid;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectCard {
    pub name: String,
    pub description: String,
    /// Content root of the project (e.g. a compose::tree_oid hex) — lets a
    /// fetcher verify it received the right tree.
    pub root_oid: String,
    /// Advertised refs: (refname, commit-oid). What a peer can fetch.
    pub refs: Vec<(String, String)>,
    /// Where the data lives: ssh:// urls, rclone remotes, dht provider ids.
    pub providers: Vec<String>,
    pub created_unix: i64,
}

impl ProjectCard {
    /// Deterministic bytes (serde_json over fixed field/element order).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("ProjectCard serializes")
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(b).map_err(|e| e.to_string())
    }

    /// Exact content id — also the DHT key for fetching this precise card.
    pub fn id(&self) -> Oid {
        Oid::hash(&self.to_bytes())
    }

    /// Discovery key: announce/look up a project by name. Multiple cards
    /// (versions, providers) can live under one name key.
    pub fn name_key(&self) -> Oid {
        Oid::hash(self.name.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> ProjectCard {
        ProjectCard {
            name: "gitmind".into(),
            description: "touch-first repo manager".into(),
            root_oid: Oid::hash(b"tree").to_hex(),
            refs: vec![("refs/heads/main".into(), Oid::hash(b"c").to_hex())],
            providers: vec!["ssh://box/srv/gitmind.git".into()],
            created_unix: 1_700_000_000,
        }
    }

    #[test]
    fn roundtrips_and_id_is_stable() {
        let c = card();
        let bytes = c.to_bytes();
        assert_eq!(ProjectCard::from_bytes(&bytes).unwrap(), c);
        assert_eq!(c.id(), ProjectCard::from_bytes(&bytes).unwrap().id());
    }

    #[test]
    fn editing_description_changes_id_but_not_name_key() {
        let a = card();
        let mut b = card();
        b.description = "different".into();
        assert_ne!(a.id(), b.id(), "content id must reflect the edit");
        assert_eq!(a.name_key(), b.name_key(), "name discovery key is stable");
    }
}
