# depot — decisions

Each entry: what was decided, what it rests on (the Linux or ecosystem
default that made it the right call), and the branch not taken. Numbered so
the manual and the code can point at them.

## 1. Data root: `/var/lib/depot` as a service, `~/.local/share/depot` as a user

**Rests on:** the Filesystem Hierarchy Standard puts *variable state that
persists across reboots* under `/var/lib/<name>`; `/var/cache` is for data
the system may delete, `/srv` for data *served* by the host ("site-specific
data") — a defensible alternative, but nothing else in the fleet uses it and
systemd's `StateDirectory=` maps only onto `/var/lib`. For an unprivileged
user the XDG Base Directory spec puts the same thing in `$XDG_DATA_HOME`,
default `~/.local/share`.

**Not taken:** `/opt/depot` (for self-contained third-party software, not
its data), `/srv/depot` (tempting because it is served, but then the
systemd unit needs a hand-rolled `ExecStartPre` to create and chown it).

## 2. systemd unit: `DynamicUser=yes` + `StateDirectory=depot`, port 8000

**Rests on:** with `DynamicUser=` systemd allocates a UID for the service's
lifetime; `StateDirectory=depot` creates `/var/lib/depot` owned by that UID
and passes it as `$STATE_DIRECTORY` (depot reads it). No user to add, no
`chown`, no way for the service to write anywhere else
(`ProtectSystem=strict`). Ports below 1024 need `CAP_NET_BIND_SERVICE`; 8000
needs nothing, so the unit drops every capability. `RestrictAddressFamilies=
AF_INET AF_INET6` — no unix sockets, no netlink.

**Not taken:** socket activation (`depot.socket`). It lets systemd hold the
port and start the service on the first connection, which is elegant, but
dnf's first request after a long idle would then pay the startup cost, and
the service is tiny anyway. Port 80 with `AmbientCapabilities=
CAP_NET_BIND_SERVICE` is a one-line change if the fleet prefers it.

## 3. Plain HTTP on the LAN; trust lives in the artifacts and the packs

**Rests on:** every artifact depot serves already carries the signature its
consumer verifies, made by the party that actually vouches for it: RPMs are
GPG-signed by Fedora (`gpgcheck=1`), crates are pinned by sha256 in
`Cargo.lock`, NARs by `NarHash` + `Sig:` in the narinfo, toolchain tarballs
by sha256 in `index.json`. TLS would authenticate *the depot*, which is the
one thing nobody needs — the depot is a cache. What TLS would add is
confidentiality of *which* packages a device fetches, on a LAN you own.

What is not covered by upstream signatures is the **catalog** — "this
version exists", "this package is in this repo" — and that is what the
ed25519 pack signature protects between the online and offline sides. Two
`depot`s that have exchanged one 32-byte key trust each other's packs;
nothing else.

**Not taken:** HTTPS with a private CA. Every device would need the CA
installed (and `cargo`, `dnf`, `nix`, `curl` each find it differently);
certificates expire, and offline boxes have wrong clocks (see 12). Kept as
a seam: `depot-httpd::Handler` does not know what listener it sits behind.

## 4. Content-addressed store, hardlinked into `pub/`, `0444`, atomic writes

**Rests on:** a hardlink is another name for the same inode; it costs a
directory entry, not space, and the file is gone only when the last name
is. So `objects/ab/<sha256>` is the truth and `pub/…/name.rpm` is a free
alias — the same RPM in two repos, or a crate that is also a pack object,
is stored once. Files are `0444` so a stray `>` cannot corrupt them
(the server needs only read). Every derived file is written as
`name.tmp-<pid>`, `fsync`ed, then `rename`d over the target: `rename(2)`
is atomic on the same filesystem, so a reader sees the old or the new file,
never a torn one — the reason `dnf` cannot observe a half-written
`repomd.xml` even under concurrent ingest.

**Not taken:** symlinks (an extra `lstat` + `readlink` per request, and
`safe_join` would have to police targets); copies (double the disk);
a database (the catalog is small enough to be one JSON file, and JSON can be
read with `jq` when the binary is not around).

## 5. dnf repositories: one repo per (distribution, release, arch); hash-named repodata

**Rests on:** a dnf repo is `repomd.xml` naming three compressed XML files
by checksum-prefixed href. dnf caches `repomd.xml` for `metadata_expire`
and refetches the others only when their names change — so naming them
`<sha256>-primary.xml.gz` (createrepo_c's default) makes every file except
`repomd.xml` immutable and cacheable forever, which is what lets depot send
`Cache-Control: immutable` for them. `Packages/<first letter>/` is the
Fedora mirror layout; nothing depends on it but people browsing.

dnf resolves across all enabled repos and takes the highest NEVRA, so
`fedora` + `updates` belong in **one** depot repo: fewer files, one
`repomd.xml` fetch, and the update supersedes automatically.

**Not taken:** `.sqlite.bz2` metadata (dnf ≥ 2 ignores it), zstd repodata
by default (dnf ≥ 4.4 reads it, but the C dependency would be the only one
in depot; `--features zstd` turns it on).

## 6. Compression: none where it is already done, gzip-1 for packs

**Rests on:** `.crate` is gzip, `.rpm` payloads are zstd (Fedora ≥ 31),
`.nar.xz` is xz, toolchain tarballs are xz. Compressing them again buys
nothing. Measured here (python gzip, the 71 `.crate` files cargo cached
to build depot, 6.4 MB): level 1 → 99.79% of the input, level 9 → 99.77%.
The level cannot matter for artifacts, so packs use level 1 and spend the
CPU only on the small text part (index lines, XML fragments, the manifest),
where level 1 already gets most of the gain (repodata XML: 26% at level 1,
23% at level 6). The real saving is the have-list: a delta pack carries only
missing objects — that is deduplication, not compression, and it is where
the order-of-magnitude comes from.

Repodata is gzip-9: it is text, it is read far more than written, and
dnf downloads it on every `metadata_expire`.

**Not taken:** zstd for packs (better ratio on the text, but a C dependency
on both sides of the air gap — the receiving side may be the minimal box);
squashfs images (great for read-only distribution, but no dedup against what
the receiver already has).

## 7. Cargo: source replacement, not a second registry

**Rests on:** `Cargo.lock` records each dependency's source id.
`[registries.depot]` gives crates a *different* source
(`registry+http://depot…`), so a lock file made online does not match one
made offline and `cargo build --locked` fails. `[source.crates-io]
replace-with = "depot"` keeps the source id `crates-io` and only changes
where bytes are fetched from — lock files are identical on both sides.
The sparse protocol (`sparse+http://`, cargo ≥ 1.68, default ≥ 1.70) fetches
one small file per crate name from a static tree, which is exactly what a
file server can offer; the git protocol would need a bare repo maintained
on every ingest.

**`net.offline` is a trap:** it forbids all network access, including the
mirror. The mirror *is* the network. Leave it off.

## 8. Rust toolchains: the standalone installers, not a rustup mirror

**Rests on:** each component tarball from `static.rust-lang.org/dist/`
carries its own `install.sh` (a plain POSIX script) that copies files into
a prefix and records them in `lib/rustlib/`. rustup runs the same scripts.
Serving the tarballs plus one `install.sh` that fetches, verifies and runs
them gives a working `rustc`/`cargo`/`rust-std` on a fresh box with only
`curl`, `tar`, `xz` and `sha256sum` — no rustup, no channel manifest, no
`$HOME/.rustup` state. Cross targets are one more `rust-std` tarball into
the same prefix.

**Not taken (yet):** mirroring the rustup dist layout so
`RUSTUP_DIST_SERVER` works unchanged. It needs the channel manifest and
date-stamped paths; it is a documented addition, not a redesign (MANUAL §10).

## 9. Nix: serve what `nix copy --to file://` writes; verify, do not re-sign

**Rests on:** a nix binary cache is static files with a defined layout, and
`nix copy` produces it. Signatures are made by whoever built the paths
(`Sig:` in the narinfo, ed25519, key named by label); the client's
`trusted-public-keys` decides. depot has no business re-signing — it
verifies `FileHash`/`FileSize` on ingest so a corrupt NAR is refused rather
than served, and sets `Priority: 30` so the LAN cache is consulted before
cache.nixos.org (40).

## 10. Packs: tar.gz, manifest first, signature over the manifest, hash-checked objects

**Rests on:** tar streams. The manifest goes first so the receiver can
refuse a pack before reading a single object; objects are named by hash so
each one is verified as it lands (the receiver hashes while writing, then
renames into place — a truncated stick fails on the object that is short,
not later inside `dnf`). The signature is over the manifest bytes, which
list every object hash, so it covers the objects transitively without
hashing the archive twice. ed25519 via `crypto-kit`: 32-byte keys, no
keyring, no agent, no expiry — the same construction nix uses.

The HTTP variant (`POST /api/pack`) writes the pack to `tmp/`, opens it,
**unlinks it**, and streams from the open descriptor: Linux keeps an
unlinked file readable until the last descriptor closes, so a client that
disconnects at 40% leaves nothing behind.

**Not taken:** zip (no streaming write; central directory at the end means
the manifest could not come first), git bundles (needs git on both sides;
the fleet's phones do not all have it).

## 11. Std-only threaded HTTP server

**Rests on:** the workload is `sendfile`-shaped: hundreds of GETs for
static, immutable files, from a handful of machines on a LAN. Eight
threads over `TcpListener` with keep-alive saturate a gigabit link for
this; an async runtime would add ~30 crates and the very thing that makes
the depot hard to bootstrap from its own mirror. Correctness that matters
for the clients is small and explicit: `ETag`/`If-None-Match` (dnf and
cargo both send it), a single byte `Range` (dnf fetches RPM headers by
range), `HEAD`, `411` for chunked bodies (uploads must declare their
length so they can be spooled), `431` for oversized headers.
`safe_join` refuses `..`, checks each component, canonicalizes against the
root, **and** applies the same check to the nearest existing ancestor for
paths that do not exist yet — the case a plain `canonicalize()` misses and
that a symlink inside `pub/` would otherwise open.

Shutdown is one flag: `SIGTERM`/`SIGINT` set it (a two-line `signal(2)`
handler via the `libc` bindings — std has no signal API), the accept loop
sees it within 100 ms, closes the work channel and joins the workers, so a
download in flight completes. Verified by killing the server mid-transfer:
the client got all 11 MB and a 200. One caveat: an *idle* keep-alive
connection holds its worker until the keep-alive timeout, so exit can lag
by that much — systemd's default `TimeoutStopSec=90s` covers it.

**Not taken:** hyper/axum (the seam is `Handler`; if HTTP/2 or TLS is
wanted, it slots in there), `net-kit`'s server (single-threaded, buffered
bodies — fine for its RPC use, wrong for 2 GB uploads).

## 12. Nothing depends on the clock

**Rests on:** boxes that have been offline for months have wrong clocks
(no NTP, dead RTC battery on a Fairphone). So: signatures do not expire;
`repomd.xml` timestamps are informational and dnf compares content hashes,
not times; packs record `created` but nothing rejects an old one; `ETag` is
mtime-based but a wrong clock only costs a cache miss.

## 13. Logging to stderr, one line per request; journald does the rest

**Rests on:** a systemd service's stderr *is* the journal. `journalctl -u
depot` gives timestamps, rotation, `--since`, `-f`, and structured filtering
for free; a log file would need `logrotate` and a path the sandboxed unit
can write. `log = false` in `depot.toml` (or `--quiet`) for a phone where
the journal is on eMMC.

## 14. musl + static for phones; page size is not the problem here

**Rests on:** postmarketOS is musl; a static musl binary runs on any
aarch64 Linux regardless of the host libc, which is the property the fleet
wants for "copy one file to the device". Apple-silicon boxes under Asahi use
16 KiB pages; that bites projects with hand-tuned allocators or jemalloc
with a baked-in page size. depot uses the system allocator and never
`mmap`s with an assumed page size, so the same source builds for both.

## 15. Names: `depot.lan`, not `depot.local`, in examples

**Rests on:** `.local` is reserved for mDNS (RFC 6762); it works only where
avahi/Bonjour runs and does not cross subnets, and an Alpine or minimal
Fedora box may not resolve it. `.lan` is not an official TLD but is what
most home routers hand out, and a real DNS entry or `/etc/hosts` line works
everywhere. `public_url` in `depot.toml` bakes the choice into every
generated client file.

## 16. Tutorials that check themselves, and diagrams that stay text

**Rests on:** two things already in the tree. voiceterm's notes wizard
gates each step with `can_advance(step, cfg) -> Result<(), String>`; a
tutorial that runs *inside* the thing it teaches can do the same, so a
step may declare `%check: <key>` and the depot evaluates it against its
own catalog and config on every request — "this depot already satisfies
this step: 2 crates, 2 versions" or "not yet: trust/ is empty". The keys
are a fixed list (`checks.rs`); a typo in a markdown file fails the test
suite rather than rendering nothing. And `nifty-core` — voiceterm's
plain-text diagram engine — gained an SVG backend, so a ` ```niftyplot `
fence in a tutorial is a diagram in the UI and readable text in `depot
tutorial` output, which is the format's first design principle.

**Not taken:** extracting the wizard's `Step` enum into a shared crate. The
generic part (next/prev clamp, index, title, a check) is a dozen lines and
the wizard's steps are wizard-specific; a shared crate would isolate
nothing that varies. The *shape* is reused, on purpose, and named here so
the next person sees the lineage. Likewise no image files: a diagram is
text in the same file as the prose, diffable, and rendered by the same
binary that serves it.
