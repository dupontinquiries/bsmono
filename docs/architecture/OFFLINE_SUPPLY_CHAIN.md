# Offline supply chain — how a LAN with no internet rebuilds its machines

The problem `apps/depot` solves, where its pieces sit in this monorepo, and
what a device on the offline side actually needs. The operator manual is
[`apps/depot/MANUAL.md`](../../apps/depot/MANUAL.md); the reasoning behind
every default is [`apps/depot/DECISIONS.md`](../../apps/depot/DECISIONS.md).
Both are embedded in the binary and served at `/manual` and `/decisions`.

## Shape

```
 ONLINE side                          air gap                 OFFLINE LAN
 ───────────                          ───────                 ───────────
 cargo fetch  ─┐                                              depot serve ──► cargo (sparse+http://depot.lan:8000/crates/index/)
 dnf download ─┼─► depot ingest ─► depot pack ──(usb/http)──► depot unpack   ──► dnf   (baseurl=http://depot.lan:8000/rpm/<repo>/)
 nix copy     ─┤        │              ▲                          │           ──► nix   (substituters = http://depot.lan:8000/nix)
 pack-rust.sh ─┘        │              └── have.txt ◄─────────────┘           ──► sh install.sh (toolchains)
                        ▼
              objects/<sha256>  (content-addressed; pub/ is hardlinks)
```

One binary, three layers, each testable alone:

| crate | job | seam |
|-------|-----|------|
| `depot-httpd` | std-only threaded HTTP/1.1 with the few features the clients need (ETag, Range, HEAD, keep-alive, traversal-safe roots) | `Handler` — hyper/TLS would replace the listener, not the routes |
| `depot-core` | store, catalog, four projections derived without upstream tools, ed25519-signed packs | pure functions over paths; the CLI and web are thin |
| `depot` | CLI + `depot serve` + embedded UI/manual | — |

## Why the projections are derived, not delegated

The depot may run on the offline side, on a phone, on Alpine. `createrepo_c`
(C, python bindings), a nix store, and rustup may not be there. So depot
reads the artifacts itself: the RPM header (four tags families and
createrepo_c's five filtering rules, pinned by byte-diffing against
createrepo_c over four reference packages), the normalized `Cargo.toml`
inside a `.crate` (pinned field-by-field against index.crates.io), the
narinfo, and the toolchain tarball name. The fixtures and where they came
from are in `apps/depot/crates/depot-core/testdata/README.md`.

## What was verified with real clients (2026-09-12)

- cargo 1.96, clean `CARGO_HOME`, source replacement: lock file's source id
  unchanged (`registry+https://github.com/rust-lang/crates.io-index`),
  fetch, build, run.
- dnf 4.14 into an `--installroot`: makecache, repoquery, `provides`,
  `install`.
- Not yet: nix (no nix in the sandbox), the toolchain installer against a
  real tarball, cross builds. `apps/depot/TODO.md`.

## Where it touches the rest of the tree

- `shared/crypto-kit` — ed25519 for pack signatures (same construction nix
  uses for narinfo `Sig:`).
- `shared/net-kit::auth` — constant-time token check for HTTP writes. Note
  its `Keyring` treats *empty* as open; depot checks the token file exists
  before building one, so "no token" means "no writes".
- `shared/cli-kit` — argument parsing.
- `maintenance/pack-rust.sh` — produces the `rust-install/` tree depot
  ingests as toolchains.
- Fleet targets (`manifest/projects.toml`): desktop, phone (musl), alpine.
  No C dependencies unless `--features zstd`.

## Linux defaults it teaches (short list; the long one is DECISIONS.md)

`/var/lib` vs `~/.local/share` (FHS vs XDG) · `DynamicUser` +
`StateDirectory` · ports ≥ 1024 need no capability · hardlinks are names for
inodes · `rename(2)` is atomic, so temp+fsync+rename is how you publish a
file · an unlinked open file stays readable · dnf refetches only what
`repomd.xml` names, so hash-named metadata is immutable · cargo source
replacement keeps `Cargo.lock` identical · `net.offline` forbids the mirror
too · `.local` is mDNS, use a real name · stderr is the journal.
