//! SRP-6a (RFC 5054 §2.6, RFC 2945 §3), the password-authenticated key
//! agreement pair-setup rides on.
//!
//! # Why this is written out rather than taken from a crate
//!
//! HAP uses SRP with SHA-512 over the 3072-bit group with g = 5, and the
//! only test vectors anyone publishes are RFC 5054's, which are SHA-1 over
//! the 1024-bit group. So the arithmetic here is generic over the digest
//! and the group, [`tests::rfc5054_appendix_b`] proves every intermediate
//! value (`x`, `k`, `v`, `A`, `B`, `u`, `S`) against the RFC with SHA-1,
//! and HAP then only changes two parameters that the RFC vectors cannot
//! check. A crate with the hash baked in would have left the arithmetic
//! itself unverifiable from here.
//!
//! # Byte conventions (RFC 5054 §2.1)
//!
//! Integers are big-endian. `PAD(x)` left-pads to the byte length of N.
//! Where the RFC hashes an unpadded value (`N`, `s`, `I`), so does this.
//! The HAP-specific choices — `H(g)` unpadded inside the client proof,
//! `K = H(S)` as the session key — follow RFC 2945 and Apple's HomeKitADK,
//! and are the two places a hardware mismatch would first show up. They
//! are named in [`Client::process`] so that if a real plug rejects M3 the
//! suspect list is two lines long.

use digest::Digest;
use num_bigint::BigUint;
use num_traits::{One, Zero};

/// A safe-prime group (N, g).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub n: BigUint,
    pub g: BigUint,
}

impl Group {
    /// Byte length of N; `PAD()` pads to this.
    pub fn n_len(&self) -> usize { (self.n.bits() as usize).div_ceil(8) }

    fn from_hex(n_hex: &str, g: u32) -> Group {
        let hex: String = n_hex.chars().filter(|c| !c.is_whitespace()).collect();
        Group { n: BigUint::parse_bytes(hex.as_bytes(), 16).expect("group prime hex"), g: BigUint::from(g) }
    }

    /// RFC 5054 Appendix A, 1024-bit, g = 2. Only for verifying the
    /// arithmetic against the RFC's vectors; HAP never uses it.
    pub fn rfc5054_1024() -> Group {
        Group::from_hex(
            "EEAF0AB9 ADB38DD6 9C33F80A FA8FC5E8 60726187 75FF3C0B 9EA2314C \
             9C256576 D674DF74 96EA81D3 383B4813 D692C6E0 E0D5D8E2 50B98BE4 \
             8E495C1D 6089DAD1 5DC7D7B4 6154D6B6 CE8EF4AD 69B15D49 82559B29 \
             7BCF1885 C529F566 660E57EC 68EDBC3C 05726CC0 2FD4CBF4 976EAA9A \
             FD5138FE 8376435B 9FC61D2F C0EB06E3",
            2,
        )
    }

    /// RFC 5054 Appendix A, 3072-bit, g = 5. What HAP pair-setup uses.
    pub fn rfc5054_3072() -> Group {
        Group::from_hex(
            "FFFFFFFF FFFFFFFF C90FDAA2 2168C234 C4C6628B 80DC1CD1 29024E08 \
             8A67CC74 020BBEA6 3B139B22 514A0879 8E3404DD EF9519B3 CD3A431B \
             302B0A6D F25F1437 4FE1356D 6D51C245 E485B576 625E7EC6 F44C42E9 \
             A637ED6B 0BFF5CB6 F406B7ED EE386BFB 5A899FA5 AE9F2411 7C4B1FE6 \
             49286651 ECE45B3D C2007CB8 A163BF05 98DA4836 1C55D39A 69163FA8 \
             FD24CF5F 83655D23 DCA3AD96 1C62F356 208552BB 9ED52907 7096966D \
             670C354E 4ABC9804 F1746C08 CA18217C 32905E46 2E36CE3B E39E772C \
             180E8603 9B2783A2 EC07A28F B5C55DF0 6F4C52C9 DE2BCBF6 95581718 \
             3995497C EA956AE5 15D22618 98FA0510 15728E5A 8AAAC42D AD33170D \
             04507A33 A85521AB DF1CBA64 ECFB8504 58DBEF0A 8AEA7157 5D060C7D \
             B3970F85 A6E1E4C7 ABF5AE8C DB0933D7 1E8C94E0 4A25619D CEE3D226 \
             1AD2EE6B F12FFA06 D98A0864 D8760273 3EC86A64 521F2B18 177B200C \
             BBE11757 7A615D6C 770988C0 BAD946E2 08E24FA0 74E5AB31 43DB5BFC \
             E0FD108E 4B82D120 A93AD2CA FFFFFFFF FFFFFFFF",
            5,
        )
    }
}

fn hash<D: Digest>(parts: &[&[u8]]) -> Vec<u8> {
    let mut h = D::new();
    for p in parts { h.update(p); }
    h.finalize().to_vec()
}

fn to_int(bytes: &[u8]) -> BigUint { BigUint::from_bytes_be(bytes) }

/// `PAD(x)`: big-endian, left-padded with zeros to `len` bytes.
pub fn pad(x: &BigUint, len: usize) -> Vec<u8> {
    let b = x.to_bytes_be();
    if b.len() >= len { return b; }
    let mut out = vec![0u8; len - b.len()];
    out.extend_from_slice(&b);
    out
}

/// `x = H(s | H(I | ":" | P))`.
pub fn compute_x<D: Digest>(salt: &[u8], identity: &[u8], password: &[u8]) -> BigUint {
    let inner = hash::<D>(&[identity, b":", password]);
    to_int(&hash::<D>(&[salt, &inner]))
}

/// `k = H(N | PAD(g))` — the SRP-6a multiplier.
pub fn compute_k<D: Digest>(group: &Group) -> BigUint {
    to_int(&hash::<D>(&[&group.n.to_bytes_be(), &pad(&group.g, group.n_len())]))
}

/// `u = H(PAD(A) | PAD(B))`.
pub fn compute_u<D: Digest>(group: &Group, a_pub: &BigUint, b_pub: &BigUint) -> BigUint {
    to_int(&hash::<D>(&[&pad(a_pub, group.n_len()), &pad(b_pub, group.n_len())]))
}

/// `v = g^x mod N` — what the accessory stores; only the simulator needs it.
pub fn verifier(group: &Group, x: &BigUint) -> BigUint {
    group.g.modpow(x, &group.n)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrpError {
    /// `B mod N == 0`: the accessory sent a degenerate public value. RFC
    /// 5054 §2.5.3 says abort; a server that does this is broken or hostile.
    DegenerateServerKey,
    /// `u == 0`: astronomically unlikely with an honest peer, fatal if it
    /// happens (the password would drop out of the shared secret).
    DegenerateU,
}

impl std::fmt::Display for SrpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SrpError::DegenerateServerKey => write!(f, "SRP: accessory sent B = 0 mod N"),
            SrpError::DegenerateU => write!(f, "SRP: scrambling parameter u = 0"),
        }
    }
}

/// What the client ends up with after processing the server's (s, B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The shared secret `S` as an integer — RFC 5054's premaster secret.
    pub premaster: BigUint,
    /// `K = H(S)`. The session key: HAP's HKDF input and the value the
    /// proofs are computed over.
    pub key: Vec<u8>,
    /// `M1 = H( H(N) xor H(g) | H(I) | s | A | B | K )`, sent in M3.
    pub client_proof: Vec<u8>,
    /// `M2 = H(A | M1 | K)`, what the accessory must send back in M4.
    pub expected_server_proof: Vec<u8>,
}

/// The controller's half of SRP-6a.
pub struct Client<D: Digest> {
    group: Group,
    private: BigUint,
    public: BigUint,
    _d: std::marker::PhantomData<D>,
}

impl<D: Digest> Client<D> {
    /// From a caller-supplied private value `a`. Tests use RFC 5054's `a`;
    /// production uses [`Client::new_random`].
    pub fn new_with_private(group: Group, private: BigUint) -> Self {
        let public = group.g.modpow(&private, &group.n);
        Client { group, private, public, _d: std::marker::PhantomData }
    }

    /// `a` from 32 random bytes — RFC 5054 §3.1 asks for at least 256 bits.
    pub fn new_random(group: Group) -> Self {
        let mut a = [0u8; 32];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut a);
        Self::new_with_private(group, to_int(&a))
    }

    pub fn group(&self) -> &Group { &self.group }

    /// `A = g^a mod N`, unpadded big-endian, for the wire.
    pub fn public_key(&self) -> Vec<u8> { self.public.to_bytes_be() }

    pub fn public_int(&self) -> &BigUint { &self.public }

    /// Given the accessory's salt and public value, derive the session and
    /// both proofs.
    ///
    /// HAP-specific choices, each the first suspect if a real accessory
    /// rejects M3: (1) `H(g)` in the client proof hashes g UNPADDED, per
    /// RFC 2945 §3 and HomeKitADK; (2) `K = H(S)` with S unpadded.
    pub fn process(&self, identity: &[u8], password: &[u8], salt: &[u8], server_public: &[u8]) -> Result<Session, SrpError> {
        let n = &self.group.n;
        let g = &self.group.g;
        let b_pub = to_int(server_public);
        if (&b_pub % n).is_zero() {
            return Err(SrpError::DegenerateServerKey);
        }
        let u = compute_u::<D>(&self.group, &self.public, &b_pub);
        if u.is_zero() {
            return Err(SrpError::DegenerateU);
        }
        let k = compute_k::<D>(&self.group);
        let x = compute_x::<D>(salt, identity, password);
        // S = (B - k * g^x) ^ (a + u * x) mod N.
        // `B - k*g^x` can be negative in the integers; reduce into [0, N).
        let kgx = (&k * g.modpow(&x, n)) % n;
        let base = ((&b_pub % n) + n - kgx) % n;
        let exp = &self.private + &u * &x;
        let premaster = base.modpow(&exp, n);
        let key = hash::<D>(&[&premaster.to_bytes_be()]);

        let hn = hash::<D>(&[&n.to_bytes_be()]);
        let hg = hash::<D>(&[&g.to_bytes_be()]);
        let hn_xor_hg: Vec<u8> = hn.iter().zip(hg.iter()).map(|(a, b)| a ^ b).collect();
        let hi = hash::<D>(&[identity]);
        let a_bytes = self.public.to_bytes_be();
        let b_bytes = b_pub.to_bytes_be();
        let client_proof = hash::<D>(&[&hn_xor_hg, &hi, salt, &a_bytes, &b_bytes, &key]);
        let expected_server_proof = hash::<D>(&[&a_bytes, &client_proof, &key]);
        Ok(Session { premaster, key, client_proof, expected_server_proof })
    }
}

/// The accessory's half, for the in-process simulator and for tests. Not
/// used by the controller. Kept beside the client so the two halves are
/// read against the same conventions.
pub struct Server<D: Digest> {
    group: Group,
    verifier: BigUint,
    private: BigUint,
    public: BigUint,
    _d: std::marker::PhantomData<D>,
}

impl<D: Digest> Server<D> {
    pub fn new_with_private(group: Group, verifier: BigUint, private: BigUint) -> Self {
        let k = compute_k::<D>(&group);
        // B = (k*v + g^b) mod N — the errata's bracketing.
        let public = (&k * &verifier + group.g.modpow(&private, &group.n)) % &group.n;
        Server { group, verifier, private, public, _d: std::marker::PhantomData }
    }

    pub fn new_random(group: Group, verifier: BigUint) -> Self {
        let mut b = [0u8; 32];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut b);
        Self::new_with_private(group, verifier, to_int(&b))
    }

    pub fn public_key(&self) -> Vec<u8> { self.public.to_bytes_be() }

    /// Verify the client's proof; on success return the server proof M2
    /// and the session key K.
    pub fn verify(&self, identity: &[u8], salt: &[u8], client_public: &[u8], client_proof: &[u8]) -> Result<(Vec<u8>, Vec<u8>), SrpError> {
        let n = &self.group.n;
        let a_pub = to_int(client_public);
        if (&a_pub % n).is_zero() {
            return Err(SrpError::DegenerateServerKey);
        }
        let u = compute_u::<D>(&self.group, &a_pub, &self.public);
        // S = (A * v^u) ^ b mod N
        let base = (&a_pub * self.verifier.modpow(&u, n)) % n;
        let premaster = base.modpow(&self.private, n);
        let key = hash::<D>(&[&premaster.to_bytes_be()]);
        let hn = hash::<D>(&[&n.to_bytes_be()]);
        let hg = hash::<D>(&[&self.group.g.to_bytes_be()]);
        let hn_xor_hg: Vec<u8> = hn.iter().zip(hg.iter()).map(|(a, b)| a ^ b).collect();
        let hi = hash::<D>(&[identity]);
        let a_bytes = a_pub.to_bytes_be();
        let b_bytes = self.public.to_bytes_be();
        let expected = hash::<D>(&[&hn_xor_hg, &hi, salt, &a_bytes, &b_bytes, &key]);
        if expected != client_proof {
            return Err(SrpError::DegenerateU); // caller maps to an auth failure
        }
        let m2 = hash::<D>(&[&a_bytes, client_proof, &key]);
        Ok((m2, key))
    }

    /// The accessory's premaster, for the RFC vector test only.
    pub fn premaster_for_test(&self, client_public: &[u8]) -> BigUint {
        let n = &self.group.n;
        let a_pub = to_int(client_public);
        let u = compute_u::<D>(&self.group, &a_pub, &self.public);
        ((&a_pub * self.verifier.modpow(&u, n)) % n).modpow(&self.private, n)
    }
}

/// Non-zero check helper for callers that hold raw bytes.
pub fn is_zero_mod(bytes: &[u8], group: &Group) -> bool {
    (to_int(bytes) % &group.n).is_zero()
}

#[allow(dead_code)]
fn one() -> BigUint { BigUint::one() }

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..clean.len()).step_by(2).map(|i| u8::from_str_radix(&clean[i..i + 2], 16).unwrap()).collect()
    }
    fn int(s: &str) -> BigUint { to_int(&hex(s)) }

    /// RFC 5054 Appendix B, verbatim. Every intermediate the RFC prints.
    #[test]
    fn rfc5054_appendix_b() {
        type H = sha1::Sha1;
        let group = Group::rfc5054_1024();
        let identity = b"alice";
        let password = b"password123";
        let salt = hex("BEB25379 D1A8581E B5A72767 3A2441EE");

        let k = compute_k::<H>(&group);
        assert_eq!(k, int("7556AA04 5AEF2CDD 07ABAF0F 665C3E81 8913186F"), "k");

        let x = compute_x::<H>(&salt, identity, password);
        assert_eq!(x, int("94B7555A ABE9127C C58CCF49 93DB6CF8 4D16C124"), "x");

        let v = verifier(&group, &x);
        assert_eq!(v, int(
            "7E273DE8 696FFC4F 4E337D05 B4B375BE B0DDE156 9E8FA00A 9886D812 \
             9BADA1F1 822223CA 1A605B53 0E379BA4 729FDC59 F105B478 7E5186F5 \
             C671085A 1447B52A 48CF1970 B4FB6F84 00BBF4CE BFBB1681 52E08AB5 \
             EA53D15C 1AFF87B2 B9DA6E04 E058AD51 CC72BFC9 033B564E 26480D78 \
             E955A5E2 9E7AB245 DB2BE315 E2099AFB"), "v");

        let a = int("60975527 035CF2AD 1989806F 0407210B C81EDC04 E2762A56 AFD529DD DA2D4393");
        let b = int("E487CB59 D31AC550 471E81F0 0F6928E0 1DDA08E9 74A004F4 9E61F5D1 05284D20");
        let client = Client::<H>::new_with_private(group.clone(), a);
        let server = Server::<H>::new_with_private(group.clone(), v, b);

        assert_eq!(client.public_key(), hex(
            "61D5E490 F6F1B795 47B0704C 436F523D D0E560F0 C64115BB 72557EC4 \
             4352E890 3211C046 92272D8B 2D1A5358 A2CF1B6E 0BFCF99F 921530EC \
             8E393561 79EAE45E 42BA92AE ACED8251 71E1E8B9 AF6D9C03 E1327F44 \
             BE087EF0 6530E69F 66615261 EEF54073 CA11CF58 58F0EDFD FE15EFEA \
             B349EF5D 76988A36 72FAC47B 0769447B"), "A");
        assert_eq!(server.public_key(), hex(
            "BD0C6151 2C692C0C B6D041FA 01BB152D 4916A1E7 7AF46AE1 05393011 \
             BAF38964 DC46A067 0DD125B9 5A981652 236F99D9 B681CBF8 7837EC99 \
             6C6DA044 53728610 D0C6DDB5 8B318885 D7D82C7F 8DEB75CE 7BD4FBAA \
             37089E6F 9C6059F3 88838E7A 00030B33 1EB76840 910440B1 B27AAEAE \
             EB4012B7 D7665238 A8E3FB00 4B117B58"), "B");

        let u = compute_u::<H>(&group, client.public_int(), &to_int(&server.public_key()));
        assert_eq!(u, int("CE38B959 3487DA98 554ED47D 70A7AE5F 462EF019"), "u");

        let premaster = int(
            "B0DC82BA BCF30674 AE450C02 87745E79 90A3381F 63B387AA F271A10D \
             233861E3 59B48220 F7C4693C 9AE12B0A 6F67809F 0876E2D0 13800D6C \
             41BB59B6 D5979B5C 00A172B4 A2A5903A 0BDCAF8A 709585EB 2AFAFA8F \
             3499B200 210DCC1F 10EB3394 3CD67FC8 8A2F39A4 BE5BEC4E C0A3212D \
             C346D7E4 74B29EDE 8A469FFE CA686E5A");
        let sess = client.process(identity, password, &salt, &server.public_key()).unwrap();
        assert_eq!(sess.premaster, premaster, "client premaster");
        assert_eq!(server.premaster_for_test(&client.public_key()), premaster, "server premaster");

        // The proofs are not in the RFC, but both sides must agree on them.
        let (m2, key) = server.verify(identity, &salt, &client.public_key(), &sess.client_proof).unwrap();
        assert_eq!(key, sess.key);
        assert_eq!(m2, sess.expected_server_proof);
    }

    /// The HAP parameters: same code, SHA-512 and the 3072-bit group. No
    /// published vectors exist; this proves the two halves agree at that
    /// size, which is what the simulator relies on.
    #[test]
    fn hap_parameters_agree_between_client_and_server() {
        type H = sha2::Sha512;
        let group = Group::rfc5054_3072();
        assert_eq!(group.n_len(), 384);
        let salt = [7u8; 16];
        let x = compute_x::<H>(&salt, b"Pair-Setup", b"031-45-154");
        let v = verifier(&group, &x);
        let client = Client::<H>::new_random(group.clone());
        let server = Server::<H>::new_random(group.clone(), v);
        let sess = client.process(b"Pair-Setup", b"031-45-154", &salt, &server.public_key()).unwrap();
        assert_eq!(sess.key.len(), 64);
        let (m2, key) = server.verify(b"Pair-Setup", &salt, &client.public_key(), &sess.client_proof).unwrap();
        assert_eq!(key, sess.key);
        assert_eq!(m2, sess.expected_server_proof);
    }

    #[test]
    fn a_wrong_setup_code_fails_the_server_side_check() {
        type H = sha2::Sha512;
        let group = Group::rfc5054_3072();
        let salt = [1u8; 16];
        let v = verifier(&group, &compute_x::<H>(&salt, b"Pair-Setup", b"031-45-154"));
        let client = Client::<H>::new_random(group.clone());
        let server = Server::<H>::new_random(group, v);
        let sess = client.process(b"Pair-Setup", b"999-99-999", &salt, &server.public_key()).unwrap();
        assert!(server.verify(b"Pair-Setup", &salt, &client.public_key(), &sess.client_proof).is_err());
    }

    #[test]
    fn a_degenerate_server_key_is_refused() {
        type H = sha2::Sha512;
        let group = Group::rfc5054_3072();
        let client = Client::<H>::new_random(group.clone());
        let zero = group.n.to_bytes_be(); // B == N ≡ 0
        assert_eq!(client.process(b"i", b"p", &[0; 16], &zero).unwrap_err(), SrpError::DegenerateServerKey);
    }
}
