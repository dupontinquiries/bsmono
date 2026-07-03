# Email sign-in: why it fails, and the exact fix (v0.1.903)

## TL;DR

Proton sign-in has never actually worked — not a regression, a
never-finished feature. Two blockers stack:

1. **`mail-crypto` is not in the default feature set.** A stock
   `cargo build` compiles the mail crypto as inert stubs, so login
   returns `CryptoUnavailable` before it even hits the network.
2. **Even with `--features mail-crypto`, the Proton SRP computation is a
   stub.** `ProtonSrp::compute` (in `mail_crypto.rs`) returns an error;
   the real SRP handshake was never wired.

This session made the *failure* honest and actionable (clear UI message
naming which blocker you hit, diagnostic `clog!` breadcrumbs) and did
the research to make blocker 2 a verified drop-in — but did NOT wire the
SRP crate into the build, because it needs a Rust toolchain bump this
sandbox can't verify against. Details below.

## What the user sees now (after this session)

- Stock build (no `mail-crypto`): the login form shows
  "This build can't sign in to Proton yet — it was compiled without the
  mail-crypto feature… Peer-to-peer messaging still works." instead of a
  dead-feeling form. `clog!("mail", "login blocked: built without
  mail-crypto…")` on the console.
- `--features mail-crypto` build: the form runs the real auth flow and
  surfaces "Proton SRP login isn't wired in this build yet (needs the
  proton-srp crate + a toolchain bump — see TODO_mail_srp_login.md)".

Peer-to-peer messaging (Track B in `docs/design/email.md`) is
independent and unaffected.

## The fix for blocker 1 (trivial, but has a real cost)

Add `mail-crypto` to `default` in `Cargo.toml`, OR document that Proton
support requires `--features mail-crypto`. NOT done automatically here
because `mail-crypto` pulls `crypto_secretbox`, `bcrypt`, `crypto_box` —
a real dependency/compile-time cost — and the maintainer may want Proton
support to stay opt-in. One-liner when you want it:

```toml
default = ["mail-crypto"]
```

## The fix for blocker 2 (the real work) — VERIFIED, ready to drop in

The intended SRP implementation is ProtonMail's own pure-Rust crate,
`proton-srp` (`github.com/ProtonMail/proton-crypto-rs`), which is exactly
the "safe path" `docs/design/email.md` §1 names ("must not be written
blind"). I read its 0.8.2 source to confirm the exact API — this wiring
is against the real crate, not guessed:

### The blocker on wiring it right now
`proton-srp` 0.8.2 is **edition 2024** (MSRV ~Rust 1.85). This project is
**edition 2021**. Pulling the crate in requires bumping the project
toolchain first. That's a project-wide decision (it affects every crate,
CI, and the reproducible-build story), and it can't be verified in this
sandbox (the pinned toolchain here is 1.75). So it's documented, not
blind-committed. Do the toolchain bump, then paste the below.

### Cargo.toml
```toml
# New opt-in feature so the toolchain-heavy dep stays isolated.
mail-proton-srp = ["mail-crypto", "dep:proton-srp"]

[dependencies]
proton-srp = { version = "0.8", optional = true, features = ["pgpinternal"] }
```
The `pgpinternal` feature gives `SRPAuth::with_pgp`, which verifies the
`/auth/info` PGP-signed modulus internally with rPGP — so we do NOT need
to pin Proton's modulus key by hand (the old TODO's worry). It's pure
Rust (no Go/cgo), so it keeps the pure-Rust build story.

### mail_crypto.rs — replace the ProtonSrp::compute stub
The API maps field-for-field onto our existing `SrpInfo` / `SrpProofs`
(verified against both sources):

`SrpInfo { version, modulus, server_ephemeral, salt, .. }` +
the user password → `SRPAuth::with_pgp(...)` → `.generate_proofs()` →
`SRPProof { client_ephemeral, client_proof, expected_server_proof }`
→ base64 (`SRPProofB64`) → our `SrpProofs` (already base64 strings).

```rust
// gated on the new feature; keep the current stub for builds without it
#[cfg(feature = "mail-proton-srp")]
impl Srp for ProtonSrp {
    fn compute(&self, password: &[u8], info: &SrpInfo) -> Result<SrpProofs> {
        use proton_srp::{SRPAuth, SrpHashVersion};
        let pw = std::str::from_utf8(password)
            .map_err(|_| MailError::Other("password not utf-8".into()))?;
        let version = SrpHashVersion::try_from(info.version as u8)
            .map_err(|e| MailError::Other(format!("bad SRP version: {e}")))?;
        // username = None is correct for hash version >= 3 (current Proton);
        // with_pgp verifies the signed modulus internally (pgpinternal).
        let auth = SRPAuth::with_pgp(
            None,
            pw,
            version,
            &info.salt,
            &info.modulus,
            &info.server_ephemeral,
        )
        .map_err(|e| MailError::Other(format!("SRP init: {e}")))?;
        let proof = auth
            .generate_proofs()
            .map_err(|e| MailError::Other(format!("SRP proof: {e}")))?;
        let b64: proton_srp::SRPProofB64 = proof.into();
        Ok(SrpProofs {
            client_ephemeral: b64.client_ephemeral,
            client_proof: b64.client_proof,
            expected_server_proof: b64.expected_server_proof,
        })
    }
    fn verify_server(&self, proofs: &SrpProofs, server_proof_b64: &str) -> Result<()> {
        // proofs.expected_server_proof is what we computed; the API sends
        // back its own. Constant-time compare (ct_eq already used elsewhere).
        if super::ct_eq(
            proofs.expected_server_proof.as_bytes(),
            server_proof_b64.as_bytes(),
        ) {
            Ok(())
        } else {
            Err(MailError::SrpServerProof)
        }
    }
}
```
And flip the suite selector so `mail-proton-srp` uses this impl while a
plain `mail-crypto` build keeps the honest stub:
```rust
srp: {
    #[cfg(feature = "mail-proton-srp")] { Box::new(ProtonSrp) }
    #[cfg(not(feature = "mail-proton-srp"))] { Box::new(UnavailableSrp) }
},
```

### crypto_available() should reflect SRP, not just sealing
Right now `MailHub::crypto_available()` is `cfg!(feature = "mail-crypto")`.
Once SRP is gated separately, login's pre-flight should check the SRP
feature specifically, e.g. add `srp_available()` = `cfg!(feature =
"mail-proton-srp")` and have `mail_login`'s blocker-1 check use that, so
a `mail-crypto`-but-not-`mail-proton-srp` build gives the blocker-2
message up front instead of after a network round-trip. (This session
left the pre-flight on `crypto_available()` since the separate feature
doesn't exist yet — update it in the same change that adds the feature.)

## After SRP: the rest of Track A (unchanged from email.md)

SRP is the gating item but not the last one. In order:
2. **PGP key unlock + read** — `proton-srp`'s sibling crates
   (`proton-crypto` / `proton-rpgp`, pure-Rust `rustpgp` backend) cover
   this; the `PgpEngine`/`KeyRing` seams already exist in mail_crypto.rs.
3. **Send** — Proton API + PGP (not raw SMTP); depends on 1+2.
4. **Receive/sync** — poll events/messages API; depends on 1+2.
5. **Plumbing** — `mail_login` does a blocking `ureq` round-trip on the
   UI thread; move it to the background once login actually runs (today
   it fails fast so it doesn't matter, but a real SRP handshake + PGP
   unlock will block the UI for a noticeable beat).

## What changed in the code this session (all verified-compiling here)

- `main.rs mail_login`: empty-field guard; `crypto_available()`
  pre-flight with an actionable message; `clog!` breadcrumbs on
  blocked/ok/failed. (Typechecked via the mail-module standalone crate.)
- `mail_crypto.rs ProtonSrp::compute`: stub error reworded from
  "wire proton-srp crate here" to a message naming the real blocker and
  pointing here.
- No behavior change for peer-to-peer messaging.

## Testing note

The SRP wiring above CANNOT be end-to-end tested without (a) the
toolchain bump and (b) a real Proton account + network. `proton-srp`
ships test vectors (`src/tests/` in the crate) — when wired, add a unit
test that runs `SRPAuth::with_pgp` against a known vector and checks the
`SrpProofs` mapping, so the trait-boundary conversion is covered even
without a live login. The live handshake itself should be verified
manually against a real account on the target hardware, per the
project's "unverifiable hot paths get on-device confirmation" discipline.

---

## v0.1.903 addendum — does bumping the toolchain break other deps? (checked)

Question raised: before wiring `proton-srp` (which needs Rust 1.85 /
edition 2024), confirm the bump doesn't break the existing dependency
tree. Checked by reading `Cargo.lock` (308 packages) and querying the
MSRV/edition metadata of every direct dep + the highest-risk transitive
crates from crates.io. Result: **the bump is safe — in fact the current
lockfile ALREADY requires Rust 1.86**, so nothing new breaks.

### Key finding: the tree already needs ≥1.86 today
The **non-optional** `ureq` HTTP dep pulls this always-compiled chain:

```
ureq 2.12.1  (core dep, not optional)
  └─ url 2.5.8
       └─ idna 1.1.0
            └─ idna_adapter 1.2.2        rust-version 1.86, EDITION 2024
                 └─ icu_normalizer 2.2.0  rust-version 1.86
                 └─ icu_properties 2.2.0  rust-version 1.86
                      └─ zerovec / tinystr / litemap / …  (1.82)
```

So a plain `cargo build` of the CURRENT code already can't be built by a
pre-1.86 toolchain. Whatever is building this project today is already
on ≥1.86. That means `proton-srp`'s edition-2024 / ~1.85 requirement is
**already satisfied** — adding it changes nothing about the toolchain
floor. (This also explains why the sandbox here, pinned to 1.75, can't
do a full `cargo build` of the project at all — unrelated to the mail
work.)

### Direct deps — all fine, all far below the floor
Every direct dependency declares an MSRV well under 1.85 (highest is
`ureq` at 1.71) and none are edition 2024 themselves:

| crate | locked ver | MSRV | edition |
|-------|-----------|------|---------|
| qmetaobject | 0.2.10 | — | 2018 |
| scraper | 0.14.0 | — | 2021 |
| ureq | 2.12.1 | 1.71 | 2018 |
| flate2 | 1.1.9 | 1.67 | 2018 |
| portable-pty | 0.8.1 | — | 2018 |
| vte | 0.13.1 | 1.62 | 2021 |
| unicode-width | 0.2.2 | 1.66 | 2021 |
| aho-corasick | 1.1.4 | 1.60 | 2021 |
| crypto_secretbox | 0.1.1 | 1.60 | 2021 |
| bcrypt | 0.15.1 | — | 2021 |
| crypto_box | 0.9.1 | 1.60 | 2021 |
| encoding_rs | 0.8.35 | 1.36 | 2018 |

The high floor comes entirely from transitive crates (icu family via
ureq→url→idna), not from anything web2qml names directly.

### The other 1.85 crates are behind optional/target-gated paths
Two locked crates need 1.85 and are edition 2024 (`hashbrown 0.17.1`,
`wit-bindgen 0.57.1`), but neither is on a default native build:
- `hashbrown 0.17.1` ← indexmap ← toml_edit ← proc-macro-crate ←
  num_enum_derive ← num_enum ← **zip ← candle-core** (`wavenet`
  feature, optional) **or ← ndk ← cpal/oboe** (Android target only).
- `wit-bindgen 0.57.1` ← wasip2 ← getrandom (WASI target only).

So even the 1.85 crates only matter on builds that already opt into
those features/targets — and 1.86 covers them anyway.

### Net conclusion
- **Bumping the declared toolchain to ≥1.86 breaks nothing** — the lock
  already implies it; you're just making the existing reality explicit.
- **Recommended:** add a `rust-toolchain.toml` pinning e.g. `1.86` (or
  current stable) so CI/dev/reproducible-builds all agree, and declare
  `rust-version = "1.86"` in `[package]`. There is NO such pin today
  (checked: no rust-toolchain.toml, no .cargo/config, no rust-version),
  which is why the floor has been drifting up invisibly as `idna`/`icu`
  bumped their MSRVs.
- Then flip `edition = "2024"` (gets resolver v3, which is MSRV-aware =
  strictly safer resolution) and add `proton-srp` per the wiring above.
- **Escape hatch if a lower floor is ever needed** (not needed for this
  work, documented for completeness): pin `idna_adapter = "=1.0.0"` in
  Cargo.toml — the 1.0 line uses the lighter `unicode-rs` backend
  instead of icu and drops that chain's MSRV to 1.57. Only relevant if
  you ever want to LOWER the floor, which is the opposite of here.

### Not verifiable in-sandbox
Couldn't run a real `cargo build` on 1.86 here (sandbox toolchain is
1.75; no rustup, and the rustup installer domain isn't allow-listed).
The above is from lockfile + crate-metadata analysis, which is exact for
MSRV/edition floors but can't catch a hypothetical source-level
compile break from a new rustc lint-turned-error. Given the lock already
implies ≥1.86 and the project presumably builds today, that risk is
near-zero, but the honest confirmation is: do one `cargo build` on the
target toolchain after bumping.
