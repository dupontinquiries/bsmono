//! pairing -- how two devices come to trust each other through a QR code on a screen.
//!
//! A device that wants to be added SHOWS an [`Offer`]: its name, both public keys, where it listens, a one-time
//! nonce, all signed by its own key so a bit flipped in the camera or the QR cannot change who it claims to be.
//! The device that SCANS it makes a [`claim`]: its own signed offer, sealed under a key derived from the X25519
//! agreement of the two devices AND the nonce. Only someone who saw the code knows the nonce, so only they can
//! make a claim the shower can open. The shower then checks the fingerprint with its owner ("trust Mara's phone,
//! 3F2A-...?") and, on yes, adds the scanner to its trust list. Nothing here trusts anyone by itself.

use crate::identity::{clean_name, Identity};
use crypto_kit::{derive_session_key, open, random_bytes, seal, verify, x25519_shared};

pub const OFFER_MAGIC: &[u8; 4] = b"JOF1";
pub const CLAIM_MAGIC: &[u8; 4] = b"JCL1";
/// An offer is good for this long (seconds), and once.
pub const OFFER_TTL_S: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    Truncated,
    BadMagic,
    BadSignature,
    /// Opening the claim failed: wrong nonce, wrong keys, or damaged.
    BadClaim,
    Expired,
    /// An offer used a second time.
    Used,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub name: String,
    pub ed_pub: [u8; 32],
    pub x_pub: [u8; 32],
    /// `host:port` where this device listens (a LAN address, or empty).
    pub addrs: Vec<String>,
    pub nonce: [u8; 16],
    /// Unix seconds when it was made.
    pub issued: u64,
}

impl Offer {
    pub fn new(me: &Identity, addrs: Vec<String>, now_s: u64) -> Offer {
        let mut nonce = [0u8; 16];
        random_bytes(&mut nonce);
        Offer { name: me.name.clone(), ed_pub: me.ed_pub, x_pub: me.x_pub, addrs, nonce, issued: now_s }
    }

    pub fn expired(&self, now_s: u64) -> bool { now_s.saturating_sub(self.issued) > OFFER_TTL_S }

    /// The bytes a QR carries: everything, then a signature over it by `me` (who must be the offer's owner).
    pub fn encode(&self, me: &Identity) -> Vec<u8> {
        let mut b = Vec::with_capacity(160);
        b.extend_from_slice(OFFER_MAGIC);
        b.extend_from_slice(&self.issued.to_be_bytes());
        let name = clean_name(&self.name);
        b.push(name.len().min(255) as u8);
        b.extend_from_slice(&name.as_bytes()[..name.len().min(255)]);
        b.extend_from_slice(&self.ed_pub);
        b.extend_from_slice(&self.x_pub);
        b.extend_from_slice(&self.nonce);
        b.push(self.addrs.len().min(8) as u8);
        for a in self.addrs.iter().take(8) {
            let a = &a.as_bytes()[..a.len().min(255)];
            b.push(a.len() as u8);
            b.extend_from_slice(a);
        }
        let sig = me.sign(&b);
        b.extend_from_slice(&sig);
        b
    }

    /// Parse and VERIFY. The signature is checked against the key inside, so this proves the bytes are intact and
    /// that whoever made them holds that key; it does not say the key is one you trust (that is the owner's yes).
    pub fn decode(b: &[u8]) -> Result<Offer, PairError> {
        let mut r = Reader { b, i: 0 };
        if r.take(4)? != OFFER_MAGIC { return Err(PairError::BadMagic); }
        let issued = u64::from_be_bytes(r.take(8)?.try_into().unwrap());
        let nlen = r.take(1)?[0] as usize;
        let name = String::from_utf8_lossy(r.take(nlen)?).into_owned();
        let ed_pub: [u8; 32] = r.take(32)?.try_into().unwrap();
        let x_pub: [u8; 32] = r.take(32)?.try_into().unwrap();
        let nonce: [u8; 16] = r.take(16)?.try_into().unwrap();
        let n = r.take(1)?[0] as usize;
        let mut addrs = Vec::new();
        for _ in 0..n.min(8) {
            let l = r.take(1)?[0] as usize;
            addrs.push(String::from_utf8_lossy(r.take(l)?).into_owned());
        }
        let signed_len = r.i;
        let sig: [u8; 64] = r.take(64)?.try_into().unwrap();
        if !verify(&ed_pub, &b[..signed_len], &sig) { return Err(PairError::BadSignature); }
        Ok(Offer { name: clean_name(&name), ed_pub, x_pub, addrs, nonce, issued })
    }
}

struct Reader<'a> { b: &'a [u8], i: usize }
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], PairError> {
        let s = self.b.get(self.i..self.i + n).ok_or(PairError::Truncated)?;
        self.i += n;
        Ok(s)
    }
}

/// The key a claim is sealed under: the two devices' X25519 agreement, bound to the offer's nonce.
fn claim_key(shared: &[u8; 32], nonce: &[u8; 16]) -> zeroize::Zeroizing<[u8; 32]> {
    let mut ikm = Vec::with_capacity(48);
    ikm.extend_from_slice(shared);
    ikm.extend_from_slice(nonce);
    derive_session_key(&ikm, b"jotter-claim-v1")
}

/// What the SCANNER sends after reading `offer`: its own signed offer (with its own nonce, so a later pairing the
/// other way round is possible) sealed under the agreed key. `my_addrs` is where it can be reached.
pub fn claim(me: &Identity, offer: &Offer, my_addrs: Vec<String>, now_s: u64) -> Vec<u8> {
    let mine = Offer::new(me, my_addrs, now_s).encode(me);
    let key = claim_key(&x25519_shared(me.x_secret(), &offer.x_pub), &offer.nonce);
    let mut out = Vec::with_capacity(4 + 32 + mine.len() + 28);
    out.extend_from_slice(CLAIM_MAGIC);
    out.extend_from_slice(&me.x_pub);
    out.extend_from_slice(&seal(&key, &mine));
    out
}

/// What the SHOWER does with a claim: open it with its own secret and the nonce it showed. `Ok` is the scanner's
/// verified offer (a name and keys to show the owner), `BadClaim` anything that does not open.
pub fn open_claim(me: &Identity, my_nonce: &[u8; 16], claim: &[u8]) -> Result<Offer, PairError> {
    let mut r = Reader { b: claim, i: 0 };
    if r.take(4)? != CLAIM_MAGIC { return Err(PairError::BadMagic); }
    let their_x: [u8; 32] = r.take(32)?.try_into().unwrap();
    let sealed = &claim[r.i..];
    let key = claim_key(&x25519_shared(me.x_secret(), &their_x), my_nonce);
    let plain = open(&key, sealed).ok_or(PairError::BadClaim)?;
    let offer = Offer::decode(&plain)?;
    // the keys named inside must be the ones that made the agreement, or someone is reusing another's claim
    if offer.x_pub != their_x { return Err(PairError::BadClaim); }
    Ok(offer)
}

/// The offers a device has shown and not yet used, so each works once and only for a while.
#[derive(Debug, Default)]
pub struct OfferBook {
    pending: Vec<([u8; 16], u64)>,
}

impl OfferBook {
    pub fn issue(&mut self, me: &Identity, addrs: Vec<String>, now_s: u64) -> Offer {
        self.pending.retain(|(_, t)| now_s.saturating_sub(*t) <= OFFER_TTL_S);
        let o = Offer::new(me, addrs, now_s);
        self.pending.push((o.nonce, now_s));
        o
    }

    /// Open `claim` against any pending offer; the matching offer is used up whether or not the owner then accepts.
    pub fn accept(&mut self, me: &Identity, claim: &[u8], now_s: u64) -> Result<Offer, PairError> {
        self.pending.retain(|(_, t)| now_s.saturating_sub(*t) <= OFFER_TTL_S);
        if self.pending.is_empty() { return Err(PairError::Expired); }
        for i in 0..self.pending.len() {
            if let Ok(o) = open_claim(me, &self.pending[i].0, claim) {
                self.pending.remove(i);
                return Ok(o);
            }
        }
        Err(PairError::BadClaim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::fingerprint;

    fn offer_for(me: &Identity) -> Offer { Offer::new(me, vec!["192.168.2.185:7471".into()], 1_000) }

    #[test]
    fn an_offer_round_trips_and_a_flipped_bit_is_caught() {
        let tablet = Identity::generate("Tablet");
        let o = offer_for(&tablet);
        let bytes = o.encode(&tablet);
        assert_eq!(Offer::decode(&bytes).unwrap(), o);
        assert!(bytes.len() < 300, "small enough for one modest QR: {}", bytes.len());
        for i in [0usize, 6, 20, 50, 90, bytes.len() - 3] {
            let mut bad = bytes.clone();
            bad[i] ^= 0x01;
            assert!(Offer::decode(&bad).is_err(), "a flipped bit at {i} is refused");
        }
        assert_eq!(Offer::decode(&bytes[..bytes.len() - 1]), Err(PairError::Truncated).or(Err(PairError::Truncated)));
        assert_eq!(Offer::decode(b"nope"), Err(PairError::BadMagic));
        // someone swapping in their own key cannot keep the old signature
        let evil = Identity::generate("Evil");
        let mut forged = bytes.clone();
        let (start, _) = (4 + 8 + 1 + o.name.len(), 0);
        forged[start..start + 32].copy_from_slice(&evil.ed_pub);
        assert!(Offer::decode(&forged).is_err());
    }

    #[test]
    fn only_someone_who_saw_the_code_can_make_a_claim_the_shower_opens() {
        let tablet = Identity::generate("Tablet");
        let phone = Identity::generate("Phone");
        let o = offer_for(&tablet);
        // the phone scanned the code (it has the offer, including the nonce)
        let c = claim(&phone, &Offer::decode(&o.encode(&tablet)).unwrap(), vec!["192.168.2.231:7471".into()], 1_001);
        let got = open_claim(&tablet, &o.nonce, &c).unwrap();
        assert_eq!((got.name.as_str(), got.ed_pub, got.x_pub), ("Phone", phone.ed_pub, phone.x_pub));
        assert_eq!(fingerprint(&got.ed_pub), phone.fingerprint(), "the owner is shown the phone's real fingerprint");
        // a stranger who knows the tablet's public keys but never saw the nonce
        let stranger = Identity::generate("Stranger");
        let guessed = Offer { nonce: [9; 16], ..o.clone() };
        let bad = claim(&stranger, &guessed, vec![], 1_001);
        assert_eq!(open_claim(&tablet, &o.nonce, &bad), Err(PairError::BadClaim));
        // damage anywhere in a claim is caught
        for i in [0usize, 10, 40, c.len() - 2] {
            let mut d = c.clone();
            d[i] ^= 1;
            assert!(open_claim(&tablet, &o.nonce, &d).is_err(), "damage at {i}");
        }
        // someone replaying the phone's sealed claim under their own key header is refused
        let mut swapped = c.clone();
        swapped[4..36].copy_from_slice(&stranger.x_pub);
        assert!(open_claim(&tablet, &o.nonce, &swapped).is_err());
    }

    #[test]
    fn an_offer_works_once_and_only_for_a_while() {
        let tablet = Identity::generate("Tablet");
        let phone = Identity::generate("Phone");
        let mut book = OfferBook::default();
        let o = book.issue(&tablet, vec![], 100);
        let c = claim(&phone, &o, vec![], 101);
        assert!(book.accept(&tablet, &c, 102).is_ok());
        assert_eq!(book.accept(&tablet, &c, 103), Err(PairError::Expired), "used once: nothing pending now");
        let o2 = book.issue(&tablet, vec![], 200);
        let c2 = claim(&phone, &o2, vec![], 201);
        assert_eq!(book.accept(&tablet, &c2, 200 + OFFER_TTL_S + 5), Err(PairError::Expired), "too late");
        // two shown at once: each claim finds its own
        let (a, b) = (book.issue(&tablet, vec![], 1000), book.issue(&tablet, vec![], 1001));
        let cb = claim(&phone, &b, vec![], 1002);
        assert!(book.accept(&tablet, &cb, 1003).is_ok());
        let ca = claim(&phone, &a, vec![], 1004);
        assert!(book.accept(&tablet, &ca, 1005).is_ok());
        assert!(o2.expired(200 + OFFER_TTL_S + 1) && !o2.expired(250));
    }
}
