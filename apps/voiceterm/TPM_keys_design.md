# TPM-bound keys design (draft)

> Status: design draft. No code yet. This documents the intended
> approach so we can iterate on the model before writing it.

## Problem

web2qml accumulates several kinds of locally-sensitive state:

- Voice fingerprint database (audio-derived, identifies speaker)
- Confusion table (learned mistranscriptions — could be a side
  channel revealing what the user says)
- Wake-word audio profile (pitch, formants — voiceprint)
- Decision cache (mapping utterances to commands)
- Command history / palette favorites

Today these live in plaintext under `~/.config/web2qml/`. Anyone with
read access to the user's home directory can read them. On a desktop
that's "any process running as the user" — which is most malware.

We want at-rest encryption with the property that even an attacker
who copies the files off the disk can't decrypt them without the
user's hardware.

## Why TPM

The TPM (or Secure Enclave on macOS, or KeyChain-bound keys on
mobile) provides:

- A hardware root of trust that won't release keys to a copy of
  the disk on a different machine
- Optional PCR-binding: keys only released when system is in a
  known good state (e.g. specific kernel version, secure-boot chain)
- Sealed storage: the encrypted blob can be on disk but only opened
  on the original hardware

Compared to an in-process derived key (e.g. argon2 over a
passphrase), TPM-bound keys give us:

1. No user passphrase prompt on every session start
2. Resilience against offline disk attacks
3. Optional auto-lock when system state changes (PCR change → seal
   becomes invalid)

## Architecture

### Three layers

```
┌─────────────────────────────────────────────────────────────┐
│ web2qml app                                                 │
│                                                             │
│  ┌──────────────┐    ┌──────────────┐   ┌──────────────┐   │
│  │ voice.rs     │    │confusion_    │   │decision_     │   │
│  │ (fingerprints│    │table.rs      │   │cache.rs      │   │
│  └──────┬───────┘    └──────┬───────┘   └──────┬───────┘   │
│         │                   │                   │           │
│         └───────────────────┼───────────────────┘           │
│                             ▼                               │
│              ┌─────────────────────────────┐                │
│              │ encrypted_store.rs (NEW)    │                │
│              │  open() / read() / write()  │                │
│              └─────────────┬───────────────┘                │
└────────────────────────────┼────────────────────────────────┘
                             ▼
                ┌──────────────────────────┐
                │ key_provider trait       │
                │  - SystemKeyring         │  (default fallback)
                │  - TpmKeyProvider        │  (Linux + tpm2-tss)
                │  - SecureEnclave         │  (macOS)
                │  - AndroidKeyStore       │  (mobile)
                └─────────────┬────────────┘
                              ▼
                ┌──────────────────────────┐
                │ Hardware key storage     │
                │  TPM / KeyChain / etc    │
                └──────────────────────────┘
```

### encrypted_store.rs

A thin wrapper around `~/.config/web2qml/<file>.encrypted`:

- On first open(): generate a random 256-bit data key, encrypt it
  with the platform key provider, store the encrypted DEK in a
  `<file>.dek` companion file.
- On subsequent open(): read the encrypted DEK, ask the key
  provider to unwrap it (TPM unseal), use the plaintext DEK to
  AES-GCM decrypt the data file.
- Writes go through a temp-file-then-rename so a power loss
  mid-write doesn't lose the file (only the new state).

Format on disk:
```
<file>.encrypted: [12-byte nonce] [N-byte ciphertext] [16-byte tag]
<file>.dek:       [TPM-sealed AES-256 DEK blob, format depends on provider]
```

### key_provider trait

```rust
pub trait KeyProvider: Send + Sync {
    /// Wrap a 32-byte data encryption key for storage.
    fn seal(&self, dek: &[u8; 32]) -> Result<Vec<u8>>;

    /// Unwrap a previously-sealed DEK.
    fn unseal(&self, sealed: &[u8]) -> Result<[u8; 32]>;

    /// Provider identifier for diagnostics.
    fn name(&self) -> &'static str;

    /// Is the provider currently available? (e.g. TPM might be
    /// powered down or not present)
    fn is_available(&self) -> bool;
}
```

Provider selection at startup:
1. Try platform-specific (TPM on Linux, SecureEnclave on macOS,
   AndroidKeyStore on Android)
2. Fall back to `SystemKeyring` (libsecret/Keychain via the OS
   credential store)
3. Last resort: `PassphraseDerived` — prompt for passphrase, derive
   key with Argon2id. Slow path; only used if user explicitly
   opts in via env var (see "no auth" note below).

### TPM provider (Linux)

Use `tpm2-tss` (already on most modern Linux distros). Keys live in
the TPM's storage hierarchy under a primary key derived from the
SRK (Storage Root Key).

The flow:
1. `seal(dek)`: create an HMAC primary key, use TPM2_Create to
   produce a sealed object containing the DEK, encrypted with the
   parent. Output is the public+private blob, ~600 bytes.
2. `unseal(blob)`: TPM2_Load the public+private blob to get a
   handle, TPM2_Unseal returns the DEK plaintext.

PCR binding is optional. Default: NO PCR binding. With PCR binding,
a kernel update would invalidate the seal — recoverable from a
backup, but a hassle. Users who care can opt in via config.

### macOS Secure Enclave

Use `security` framework's `SecKeyCreateRandomKey` with
`kSecAttrTokenIDSecureEnclave`. Keys can't leave the enclave; we
do encryption inside it via `SecKeyCreateEncryptedData`.

### Android KeyStore

`AndroidKeyStore` provider: store an AES-GCM key with `setUserAuthenticationRequired(false)` (we don't want a biometric
prompt every time we read a fingerprint). The key is hardware-backed
on devices with Strongbox/Titan/SE.

### SystemKeyring fallback

`secret-tool` (libsecret) on Linux, `Security.framework` keychain
on macOS, `wincred` on Windows. Less secure than TPM (the keychain
key is unlocked when the user logs in, accessible to any process
under their UID) but better than plaintext.

### "No auth" note

Some users (servers, embedded devices, ad-hoc test setups) won't
have any of the above. For them: env var `WEB2QML_NO_ENCRYPTION=1`
disables encryption entirely with a clear console warning. Files
stay in plaintext. Documented as a footgun.

## Migration

Existing plaintext files migrate at first start:
1. Detect plaintext file present, encrypted version absent
2. Read plaintext, write encrypted version
3. Rename plaintext to `<file>.plaintext-backup` (don't delete
   immediately — give user one session to recover if encryption
   provider mis-detects)
4. Next session: confirm encrypted version is readable, then delete
   the .plaintext-backup

Migration logged to stderr clearly so user knows their data is now
encrypted.

## Threat model

In scope:
- Offline disk attacks: attacker gets the encrypted files but not
  the original hardware → can't decrypt
- Cross-machine copy: same files on different machine → can't
  decrypt
- Process running as user without TPM access (e.g. browser
  sandbox): can't unseal the DEK directly

Out of scope:
- Process running as user WITH TPM access: identical security
  posture to running web2qml itself. Mitigation: keep web2qml
  small, audit its dependencies.
- Live RAM dumps: DEKs and decrypted data live in process memory.
  We use `mlockall` and zeroize on drop where practical, but a
  privileged attacker can read process memory. Not solvable
  in user-mode software.
- Compromised TPM firmware: vendor's responsibility.
- User accidentally exporting the DEK via debug logging: we don't
  log key material.

## Open questions

1. **Per-file keys or single master?** Single master is simpler;
   per-file lets us revoke one type of data without losing others.
   Lean: single master at first, per-file as an optimization later.

2. **Key rotation?** No rotation in v1. If keys are compromised,
   user runs `web2qml --reset-keys` and we re-encrypt everything
   under fresh keys.

3. **Wrap with passphrase as well?** Defense in depth: TPM unseal
   returns a key, that key is XOR'd with passphrase-derived key.
   Both required to decrypt. Adds friction. v2 if requested.

4. **What about the swap?** If a sensitive page gets swapped,
   it lands on disk plaintext. Only mlockall avoids this, and
   mlockall has overhead. Possible: lock just the DEK and
   per-file decrypted hot pages.

## Implementation phases

**Phase 1**: encrypted_store.rs + SystemKeyring provider only.
Get the encryption pipeline working end-to-end with the easiest
provider first. Test migration. Document.

**Phase 2**: TPM provider. Behind a feature flag. Detect TPM via
`/dev/tpm0` or `tpm2_getcap`. Fall through to SystemKeyring if
absent.

**Phase 3**: Per-platform providers (macOS Secure Enclave,
Android KeyStore). Compile-time feature gates.

**Phase 4**: PCR binding (opt-in). Key rotation.

## Dependencies considered

- `aes-gcm` (RustCrypto): minimal, well-audited, no_std-compatible
- `tpm2-tss` (via `tss-esapi` crate): C bindings, big, but standard
- `keyring` (Rust): cross-platform System credential store, but the
  abstraction leaks platform quirks; might write our own thin shim
- `argon2` (RustCrypto): for passphrase fallback
- `zeroize` (RustCrypto): wipe-on-drop for keys

Likely yes: aes-gcm, argon2, zeroize. Likely no for v1: tss-esapi
(C bindings can fail in unfortunate ways; deferred until tested).
