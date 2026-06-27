# Email: Proton bridge + in-house peer messaging

Two tracks. Track A bridges a real ProtonMail account. Track B is the
in-house, device-as-provider system where w2q users address each other
by public key. They share the compose UI and the at-rest crypto, but
are otherwise independent — Track B does **not** depend on Track A
working.

## Track A — ProtonMail bridge: what's still needed

The module seams exist (`mail_crypto.rs` traits + `mail_*` providers);
what's missing is the real implementations behind them. In dependency
order:

1. **SRP login** (`Srp` trait). Proton uses a bespoke SRP variant:
   little-endian padding, a custom `expandHash`, and a `$2y$` bcrypt
   of the password as the inner hash. The `/auth/info` modulus is
   PGP-signed and must be verified against Proton's published modulus
   key before use. Until this lands, `login()` returns
   `CryptoUnavailable`, so nothing downstream can run. This is the
   gating item. Safe paths to implement it: the `proton-srp` crate, or
   a port validated against Proton's published test vectors. It must
   not be written blind — a wrong SRP is a silent auth/security break.

2. **PGP key unlock + read** (`PgpEngine`, `KeyRing`). After login,
   address-key tokens are decrypted with the user key, then message
   bodies are PGP-decrypted. Needs a vetted OpenPGP impl (sequoia or
   rpgp). Read path can land before write.

3. **Send** (the part flagged in chat). Proton has no plain SMTP from
   outside the official Bridge; sending goes through the Proton API:
   build a MIME message, PGP-encrypt/sign to recipients (or send
   "external" for non-Proton addresses per the account's settings),
   and POST to the messages endpoint. So "web SMTP" here really means
   "the Proton send API + PGP," not a raw SMTP socket. Depends on 1+2.

4. **Receive/sync**. Poll the events/messages API (Proton has no public
   IMAP outside Bridge), decrypt with 2, surface via the existing inbox
   pane. Depends on 1+2.

5. **Plumbing**: `mail_login` does a blocking `ureq` round-trip on the
   UI thread — move to async/background once login actually runs.

## Track B — In-house peer messaging (`peer_mail.rs`)

Each device is its own provider. A user's **address is their public
key**. Built this iteration:

- **Identity**: an X25519 keypair (`Identity::load_or_create`). The
  public key is the address; `fingerprint()` is a short display form.
- **Keyring**: handle → public key (`Keyring`, JSON at
  `~/.config/web2qml/mail/peers/keyring.json`).
- **Sealed envelope** (`Envelope`): a transport-agnostic blob —
  `w2qmsg/1` + sender pubkey + nonce + ciphertext, all base64. Rides
  over any channel.
- **Crypto** (`seal`/`open`): NaCl `crypto_box` (X25519 →
  XSalsa20-Poly1305). `seal` encrypts to the recipient's public key and
  authenticates the sender; `open` succeeds only against the sender key
  the envelope carries. So the recipient learns both "for me" and "from
  that peer." Gated behind `mail-crypto` (stub returns `Unavailable`
  when off), mirroring Track A. Verified: roundtrip, wrong-recipient
  rejection, tamper detection, sender-forgery rejection.

### Next steps for Track B

- **Wire the UI**: a compose-to-peer path (pick a keyring handle →
  `seal` → hand the envelope to transport), a "my address" view
  (show/share the device public key), and keyring management
  (add/scan a contact's key). The sealed envelope can reuse the draft
  store for outbox/queueing.
- **Seal the identity at rest** (SECURITY TODO): the secret key is
  currently stored base64 in `identity.json`. Wrap it with the
  existing machine-bound `PolicySecretBox` before this leaves
  prototype — the seam is already in `mail_secure.rs`.
- **Transport / rendezvous** (deferred by request — the user has a
  plan for locating peers): the envelope format is intentionally
  carrier-agnostic so this can be added without touching the crypto.
- **Signing vs. encryption keys**: today one X25519 key does
  authenticated encryption. If detached signatures or at-rest
  verification are wanted later, add an Ed25519 signing key alongside.
