# depot

An offline mirror for a LAN without internet: cargo crates (sparse
registry), Fedora RPMs (dnf repodata), a Nix binary cache, and Rust
toolchains — served by one static binary from one directory, and moved
between the online and offline sides as signed, content-deduplicated packs.

- **Manual:** [MANUAL.md](MANUAL.md) — also served by the running depot at
  `/manual` and printed by `depot manual`.
- **Why it is built this way:** [DECISIONS.md](DECISIONS.md) — the Linux
  and ecosystem defaults each choice rests on (`/decisions`, `depot decisions`).
- **Tutorials:** [tutorials/](tutorials/) — six step-by-step guides (Linux
  basics, server setup, filling the depot, sync, clients, USB link), shown
  one step at a time in the UI with progress (`/tutorials`, `depot tutorial`).
  Steps can carry `%check: <key>` — a live check against the running depot
  (the notes wizard's `can_advance`, applied to a tutorial) — and
  ` ```niftyplot ` diagrams rendered by `shared/nifty-core`'s SVG backend.

## Layout

```
crates/depot-httpd   std-only threaded HTTP/1.1 — the transport seam (hyper/TLS would slot in here)
crates/depot-core    headless: store, catalog, the four projections, signed packs
crates/depot         the binary: CLI + `depot serve` (routes in web.rs) + embedded UI (ui/)
tutorials/           markdown, one `## ` per step, optional `%check:` and niftyplot fences; parsed by crates/depot/src/tutorial.rs
crates/depot/src/checks.rs   the live check keys (initialised, public_url_set, rpm_keys, trusted_any, …)
crates/depot/src/terminal.rs the pty session manager behind the in-browser terminal (opt-in)
tools/ui-test.js     executes the UI under jsdom against a live server (27 checks)
systemd/             hardened unit: DynamicUser + StateDirectory=depot, port 8000, no capabilities
flake.nix            nix build (unverified here — no nix in the sandbox)
```

The projections are derived **without the upstream tools**: no `createrepo_c`,
no `cargo-index` helper, no `nix` on the depot. The RPM and crate output is
tested byte-for-byte / field-for-field against those tools' real output
(`crates/depot-core/testdata/README.md` records the provenance).

## Quick start

```sh
cargo build --release
target/release/depot init                       # ~/.local/share/depot (or /var/lib/depot as root)
target/release/depot ingest crate ~/.cargo/registry/cache/*/
target/release/depot ingest rpm --repo fedora-42-x86_64 ./rpms
target/release/depot serve                      # http://<host>:8000/
```

Then on a device: `curl -fsSL http://<host>:8000/bootstrap.sh?all=1 | sh`
(read it first — it prints every file before writing it).

## Verification (2026-09-12)

Built and tested in a sandbox with the offline Rust 1.96 drop, `rpm`,
`rpmbuild`, `createrepo_c` and Ubuntu's `dnf` 4.14 from apt.

- `cargo test`: depot-httpd 8/8, depot-core 33/33, depot 2/2; no warnings.
- **RPM repodata** is byte-identical to `createrepo_c`'s for four packages
  (rpm upstream's `hello`/`foo` fixtures and an edge-case RPM built here).
  Break-testing three of the replicated filter rules fails the tests.
- **Crate index lines** are field-equal to index.crates.io's for two crates.
- **Real cargo** (clean `CARGO_HOME`, source replacement): resolves, locks
  with the crates-io source id unchanged, fetches, builds, runs.
- **Real dnf** (`--installroot`): makecache, repoquery, `provides` by path,
  `install` from the depot.
- **Toolchains, real tarballs** (the 1.96.0 x86_64 set from `pack-rust.sh`):
  ingest, then the served `install.sh` into a fresh prefix — sha256 checked
  against `index.json`, three components installed, `rustc 1.96.0`; that
  toolchain then built the depot-fetched crates. Breaking the computed hash
  makes the installer refuse and install nothing.
- **HTTP writes** with the admin token: PUT crate (fresh, then duplicate →
  `fresh:false`), PUT rpm, PUT a real 10 MB toolchain tarball (hash matches
  the CLI path's), junk as a crate → 422 and the object is removed, wrong
  token → 401. Uploads stream into the store without the catalog lock.
- **UI executed** under jsdom against the live server (`tools/ui-test.js`):
  31 checks across every view, including the Clients tab's live refresh,
  the tutorial stepper (progress, mark-done, next/previous, `/tutorials` as
  a plain URL), a niftyplot fence rendering to inline SVG, and live check
  badges in both states.
- **Diagrams** rasterised with `rsvg-convert` and viewed: the SVG backend's
  output is correct outside a browser too (first cut was not — `var()`
  only — and that is recorded in the nifty-core commit).
- **In-browser terminal** (opt-in `[terminal]`): a real shell behind a pty on
  the depot host, admin-token-gated, short-polled; verified end to end over
  HTTP (`uname`/`whoami`/`exit`, resize, reap) and under jsdom
  (`tools/ui-term-test.js`, 8 checks: launcher, session, keycap, run/copy
  buttons, close). Reuses voiceterm's `portable-pty` and keycap sequences.
- **SIGTERM** mid-download: the download completes, the process exits.
- curl: ETag/304, Range/206, HEAD, traversal refused, writes refused without
  the admin token.
- `systemd-analyze verify` passes; offline security exposure 1.2.

Not verified: nix client, `flake.nix`, cross builds (musl/aarch64), a real
browser (jsdom only). See [TODO.md](TODO.md).
