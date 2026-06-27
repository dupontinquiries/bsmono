//! mailbox.rs — the opinionated shared mailbox over the DHT-as-host two-layer
//! model. An app supplies a `namespace` + `id`; posting seals a message under a
//! group message key, stores it as a content-addressed blob, and appends its
//! pointer to the current epoch's feed. Reading polls the feed and fetches the
//! referenced blobs. Durability is a property of POPULATION: a mailbox shared by
//! many members is durable because many nodes hold its buckets — the
//! mobile-network-switch superpower (your data survives because the group does).
//!
//! Built on snapcore (the keyspace + store) and crypto-kit (sealing + the group
//! `SenderChain`). The storage [`Backend`] is a seam: snapcore's local `Store`
//! backs the unit tests; the networked DHT (iterative lookup + store-to-closest
//! over a `Transport`, plus cluster replication for `Durable`) is the production
//! impl and the one network seam not exercised here.

use crate::keys;
use crypto_kit::{open, seal};
use snapcore::store::{Key, Store};

/// Durability profile — the DeliveryHint, condensed. `Durable` lives long in the
/// DHT and is worth replicating onto the cluster roster (durability-through-
/// population); `Ephemeral` is short-lived and single-store (a frame, a transient
/// control message). The app's routing survey decides which a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifetime {
    Durable,
    Ephemeral,
}

impl Lifetime {
    /// TTL in seconds for stored values (refreshed in place by re-announce).
    pub fn ttl_secs(self) -> u64 {
        match self {
            Lifetime::Durable => 30 * 24 * 3600, // ~30 days
            Lifetime::Ephemeral => 300,          // 5 minutes
        }
    }
    /// Whether to replicate onto the cluster roster (snapcore `replicate_to_cluster`).
    pub fn replicate(self) -> bool {
        matches!(self, Lifetime::Durable)
    }
}

/// The storage seam: put/get values at a keyspace point. snapcore's local `Store`
/// implements it directly (below); the networked DHT backing is the production
/// impl (the network seam).
pub trait Backend {
    fn put(&mut self, key: Key, value: Vec<u8>, ttl: u64, now: u64);
    fn get(&self, key: &Key, now: u64) -> Vec<Vec<u8>>;
}

impl Backend for Store {
    fn put(&mut self, key: Key, value: Vec<u8>, ttl: u64, now: u64) {
        Store::put(self, key, value, ttl, now)
    }
    fn get(&self, key: &Key, now: u64) -> Vec<Vec<u8>> {
        Store::get(self, key, now)
    }
}

/// A posting envelope, stored as a content-addressed blob: who sealed it + which
/// `SenderChain` index they used + the ciphertext. A reader uses `(sender,
/// key_index)` to pick the sender's chain and derive the matching message key,
/// then `open`s. Binary length-prefixed — the ciphertext is arbitrary bytes, not
/// text, so this is its own framing, not the text `net-kit::wire` codec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posting {
    pub sender: String,
    pub key_index: u32,
    pub ciphertext: Vec<u8>,
}

impl Posting {
    pub fn to_bytes(&self) -> Vec<u8> {
        let s = self.sender.as_bytes();
        let mut o = Vec::with_capacity(12 + s.len() + self.ciphertext.len());
        o.extend_from_slice(&(s.len() as u32).to_le_bytes());
        o.extend_from_slice(s);
        o.extend_from_slice(&self.key_index.to_le_bytes());
        o.extend_from_slice(&(self.ciphertext.len() as u32).to_le_bytes());
        o.extend_from_slice(&self.ciphertext);
        o
    }

    pub fn from_bytes(b: &[u8]) -> Option<Posting> {
        let mut p = 0usize;
        let slen = take_u32(b, &mut p)? as usize;
        let sender = String::from_utf8(b.get(p..p + slen)?.to_vec()).ok()?;
        p += slen;
        let key_index = take_u32(b, &mut p)?;
        let clen = take_u32(b, &mut p)? as usize;
        let ciphertext = b.get(p..p + clen)?.to_vec();
        Some(Posting { sender, key_index, ciphertext })
    }
}

fn take_u32(b: &[u8], p: &mut usize) -> Option<u32> {
    let v: [u8; 4] = b.get(*p..*p + 4)?.try_into().ok()?;
    *p += 4;
    Some(u32::from_le_bytes(v))
}

/// A shared mailbox identified by an app-supplied `namespace` + `id`. The two
/// bucket families (per-epoch feed of pointers, content-addressed blobs) are
/// derived in [`crate::keys`]; the methods here are the post/read flow over a
/// [`Backend`].
#[derive(Clone, Debug)]
pub struct Mailbox {
    pub namespace: String,
    pub id: String,
}

impl Mailbox {
    pub fn new(namespace: &str, id: &str) -> Self {
        Self { namespace: namespace.to_string(), id: id.to_string() }
    }

    /// Seal `plaintext` under `message_key` (the sender's `SenderChain` key at
    /// `key_index`), store it as a content-addressed blob, and append its pointer
    /// to `epoch`'s feed. Returns the blob's content hash.
    #[allow(clippy::too_many_arguments)]
    pub fn post<B: Backend>(
        &self,
        store: &mut B,
        sender: &str,
        key_index: u32,
        message_key: &[u8; 32],
        plaintext: &[u8],
        epoch: u64,
        life: Lifetime,
        now: u64,
    ) -> String {
        let ciphertext = seal(message_key, plaintext);
        let blob = Posting { sender: sender.to_string(), key_index, ciphertext }.to_bytes();
        let ch = keys::content_hash(&blob);
        let ttl = life.ttl_secs();
        store.put(keys::blob_key(&ch), blob, ttl, now);
        store.put(
            keys::feed_key(&self.namespace, &self.id, epoch),
            ch.clone().into_bytes(),
            ttl,
            now,
        );
        ch
    }

    /// Poll `epoch`'s feed and fetch every referenced blob, returning the
    /// (still-sealed) postings.
    pub fn fetch<B: Backend>(&self, store: &B, epoch: u64, now: u64) -> Vec<Posting> {
        let mut out = Vec::new();
        for ptr in store.get(&keys::feed_key(&self.namespace, &self.id, epoch), now) {
            let Ok(ch) = std::str::from_utf8(&ptr) else { continue };
            for blob in store.get(&keys::blob_key(ch), now) {
                if let Some(p) = Posting::from_bytes(&blob) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Fetch and open: `keyfn(sender, index)` resolves the message key for each
    /// posting — typically `SenderChain::from_root(root).message_key_for(index)`
    /// for the sender's distributed epoch root. Skips postings whose key can't be
    /// resolved or whose authentication fails.
    pub fn fetch_open<B: Backend>(
        &self,
        store: &B,
        epoch: u64,
        now: u64,
        mut keyfn: impl FnMut(&str, u32) -> Option<[u8; 32]>,
    ) -> Vec<Vec<u8>> {
        self.fetch(store, epoch, now)
            .iter()
            .filter_map(|p| keyfn(&p.sender, p.key_index).and_then(|k| open(&k, &p.ciphertext)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_kit::group::SenderChain;

    #[test]
    fn posting_envelope_roundtrips() {
        let p = Posting { sender: "alice".into(), key_index: 7, ciphertext: vec![1, 2, 3, 0, 255] };
        assert_eq!(Posting::from_bytes(&p.to_bytes()), Some(p));
        assert!(Posting::from_bytes(b"\x05\x00").is_none(), "truncated → None");
    }

    #[test]
    fn lifetime_ttl_and_replicate() {
        assert!(Lifetime::Durable.ttl_secs() > Lifetime::Ephemeral.ttl_secs());
        assert!(Lifetime::Durable.replicate() && !Lifetime::Ephemeral.replicate());
    }

    #[test]
    fn post_then_read_recovers_plaintext_for_a_fresh_reader() {
        // sender side: a fresh epoch sender key (index 0), distributed as `root0`
        let mut chain = SenderChain::generate();
        let root0 = chain.export_root(); // the sender key a member seals to co-members at epoch start
        let (idx, mk) = chain.next_message_key(); // (0, mk0); chain advances to index 1

        let mut store = Store::new();
        let mb = Mailbox::new("group-feed", "g:42");
        let ch = mb.post(&mut store, "alice", idx, &mk, b"hello world", 7, Lifetime::Durable, 1_000);

        // a fresh reader holding only the mailbox id + alice's epoch root recovers it
        let opened = mb.fetch_open(&store, 7, 1_000, |sender, i| {
            if sender == "alice" {
                SenderChain::from_root(root0).message_key_for(i)
            } else {
                None
            }
        });
        assert_eq!(opened, vec![b"hello world".to_vec()]);
        // the blob is content-addressed at blob_key(content_hash)
        assert!(!store.get(&keys::blob_key(&ch), 1_000).is_empty());
    }

    #[test]
    fn next_epoch_feed_is_empty_until_posted() {
        // epoch is in the feed key: a rekey to epoch+1 starts an empty feed, so a
        // removed member who can still compute the epoch+1 key string reads nothing.
        let mut chain = SenderChain::generate();
        let root0 = chain.export_root();
        let (idx, mk) = chain.next_message_key();
        let mut store = Store::new();
        let mb = Mailbox::new("group-feed", "g:7");
        mb.post(&mut store, "bob", idx, &mk, b"epoch 0 msg", 0, Lifetime::Durable, 10);

        // nothing posted to epoch 1 yet
        assert!(mb.fetch(&store, 1, 10).is_empty(), "next epoch's feed is empty");
        // epoch 0 still readable
        let got = mb.fetch_open(&store, 0, 10, |s, i| {
            if s == "bob" {
                SenderChain::from_root(root0).message_key_for(i)
            } else {
                None
            }
        });
        assert_eq!(got, vec![b"epoch 0 msg".to_vec()]);
    }
}
