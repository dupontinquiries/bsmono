# Sharing / transport roadmap

How the gitmind GUI moves a repo between devices today, and where it's going when
`sharekit` lands. This is a design note, not a spec — it exists so the pieces being
built now line up with the eventual ones instead of being throwaway.

## The transport seam

A peer's repo is advertised as an `announce::ProjectCard` whose `providers` are
**scheme-prefixed URLs**. Install dispatches on the scheme:

| scheme   | status        | how it moves bytes                                  |
|----------|---------------|-----------------------------------------------------|
| `ssh://` | **wired**     | `git clone` over ssh (non-interactive; key + host-key trust handled in-GUI) |
| `bt://`  | reserved      | Bluetooth OBEX push of a bundle (the *send* half exists today as a direct action; not yet a card provider) |
| `dht://` | reserved      | fetch via opendht / snapclone device IDs            |

`netshare::install_card` already returns a clear "not wired yet" for `bt://` /
`dht://` so adding them later is additive, not a rewrite.

## What exists now (this GUI, `apps/gitmind/crates/gui`)

- `netshare.rs` — `ProjectCard` advertise (HTTP) / lookup / `ssh://` install, built
  on `gitmind-core::announce` + `net-kit::http`.
- `deploy.rs` — ssh discovery / browse / push / remote build, **non-interactive ssh**
  (BatchMode + ConnectTimeout + accept-new), host-key trust (`trust`/`re-trust`),
  key setup via password→`SSH_ASKPASS`, and **two-sided fingerprint matching**
  (`local_*_pubkey` + `fetch_peer_key` + `key_fingerprint_art` → show SHA256 +
  randomart on both screens, then `write_known_host` trusts exactly what was shown).
- `bttransfer.rs` — Bluetooth OBEX push (`bt-obex`/`obexftp`/`bluetooth-sendto`),
  used by the Network tab's "send over Bluetooth" panel to hand a bundle to a
  paired device with no shared LAN.

## Where `sharekit` comes in (eventual)

`sharekit` is the future shared crate that **wraps the gitmind sharing code** so it
isn't bolted to the GUI:

1. **Extract** `ProjectCard` advertise/lookup/install (currently `netshare.rs`) +
   the transport dispatch into `shared/sharekit`, over `announce` + `net-kit`.
   `gitmind-gui` then depends on `sharekit` instead of carrying `netshare.rs` inline.
2. **Device identity**: stable per-device IDs via **opendht** (we already have an
   `opendht` project) — a `dht://<device-id>/<repo>` provider so peers find each
   other without typing `host:port`. `snapclone` becomes a transport that resolves a
   device ID → a reachable endpoint and streams the repo.
3. **Bluetooth as a provider**: promote `bttransfer` from a direct action to a
   `bt://<bdaddr>` provider so a card can advertise "reachable over Bluetooth too".

The seam (scheme-prefixed providers + per-scheme install) is the contract; sharekit
fills in `dht://`/`bt://` and owns the card/transport code that gitmind currently
hosts. Nothing above needs the GUI to change shape — only to depend on sharekit and
register the new schemes.

## Verification reality

The ssh / Bluetooth / keyscan I/O all need real devices/radios; the sandbox has
none, so those run on-device. What's machine-tested is the pure logic: provider
scheme dispatch, fingerprint/summary parsing, `bluetoothctl` parsing, OBEX command
construction, and the no-network guards.
