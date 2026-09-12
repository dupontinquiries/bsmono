# depot — manual

depot is a mirror for a LAN that has no internet. It holds cargo crates, Fedora
RPMs, a Nix binary cache and Rust toolchains; serves them over plain HTTP in
exactly the layouts `cargo`, `dnf`, `nix` and a shell script expect; and moves
content between the online and offline sides as signed, deduplicated **packs**.

It is one static binary and one directory. There is no database, no daemon
other than the web server, and nothing it writes is in a format you cannot
read with `ls`, `cat` and `sha256sum`.

This manual is served by the running depot at `/manual`, and printed by
`depot manual`. The reasoning behind the defaults is in `DECISIONS.md`
(`/decisions`, `depot decisions`). If you would rather be walked through
it, the **Tutorials** tab (`/tutorials`, `depot tutorial`) has six
step-by-step guides — Linux basics, setting up the server, filling it,
syncing across the air gap, preparing clients, serving over USB — each
step with the commands to run and what you should see. Where a step is
something the depot can verify about itself (a key attached, a URL set,
content present) the step shows whether *this* depot already satisfies
it; diagrams are drawn from text fences by the same binary.

---

## 1. Install

### Build

```sh
cd apps/depot
cargo build --release                       # → target/release/depot
# for a phone or an Alpine box:
cargo build --release --target aarch64-unknown-linux-musl
```

Dependencies are deliberately few (no async runtime, no TLS, no C except
optionally zstd), so a depot can be rebuilt **from its own mirror** once the
crates are in it — the bootstrap property.

### Where the data lives

| running as | root                        | why                                        |
|-----------|------------------------------|--------------------------------------------|
| root / a system service | `/var/lib/depot` | FHS: variable state of a service (§DECISIONS 1) |
| a user     | `~/.local/share/depot`      | XDG data dir                               |
| anything   | `--root DIR` / `$DEPOT_ROOT` | explicit always wins                       |

```sh
depot init                 # creates the root, depot.toml, a signing key
depot serve                # http://0.0.0.0:8000/
```

Open `http://<host>:8000/` — the UI, this manual, and every artifact tree.

### As a service

`systemd/depot.service` runs it as a dynamic user with `StateDirectory=depot`
(so the root is `/var/lib/depot`, owned by the service, created on first
start):

```sh
sudo install -Dm755 target/release/depot /usr/local/bin/depot
sudo install -Dm644 systemd/depot.service /etc/systemd/system/depot.service
sudo systemctl enable --now depot
journalctl -u depot -f                    # requests are logged one per line
```

Then, as root, `depot init --root /var/lib/depot` is done for you on first
start; use `sudo -u depot depot ... --root /var/lib/depot` or
`DEPOT_ROOT=/var/lib/depot` for the CLI.

### Firewall

Port 8000/tcp must be open on the LAN interface only:

```sh
# Fedora / firewalld — the zone your LAN NIC is in (usually FedoraServer or public)
sudo firewall-cmd --permanent --add-port=8000/tcp && sudo firewall-cmd --reload
# nftables, by hand
sudo nft add rule inet filter input tcp dport 8000 accept
```

Do **not** expose it to the internet: the only authentication is the admin
token for writes, and reads are open by design (§DECISIONS 3).

### Naming the host

Devices need a stable name. Pick one:

- **mDNS** (`depot.local`): works out of the box on Fedora (avahi) and most
  phones; zero config; does not cross subnets.
- **A LAN DNS entry / `/etc/hosts`** (`depot.lan`): survives subnets and
  hosts without avahi (Alpine/musl boxes often lack it). Recommended; set
  `public_url = "http://depot.lan:8000"` in `depot.toml` so generated
  configs use it regardless of how you opened the UI.

---

## 2. Concepts

```
<root>/
  depot.toml          settings (annotated; every key has a default)
  catalog.json        what is here and where it came from — the only "database"
  objects/ab/<sha256> every artifact and fragment, once, read-only (0444)
  pub/                what the web server serves; HARDLINKS into objects/
    crates/{index/{config.json,…},dl/…}
    rpm/<repo>/{repodata/,Packages/,RPM-GPG-KEY}
    nix/{nix-cache-info,<hash>.narinfo,nar/…}
    toolchains/{index.json,install.sh,<ver>/<triple>/…}
  keys/depot.key      this depot's signing key (0600)   — never leaves the box
  keys/depot.pub      the matching public key           — give it to receivers
  keys/admin.token    if present, enables HTTP writes
  trust/<name>.pub    public keys whose packs `unpack` accepts
  tmp/                in-flight uploads and packs
```

**Objects** are content-addressed: the file's sha256 is its name. Ingesting
the same `.crate` twice costs nothing; the same RPM in two repos is stored
once; a pack never carries a byte the receiver already has.

**pub/ is disposable.** `depot rebuild` regenerates it from `catalog.json` +
`objects/`. Back up those two; nothing else matters.

**Packs** (`*.depot`) are `tar.gz`: a manifest (catalog subset + object list),
an ed25519 signature over it, then the objects. `unpack` checks the signature
before reading any object, and every object against its name as it lands.

---

## 3. Gathering content (the online side)

You need an internet-connected machine with the same tools the offline boxes
use. It does not have to be Fedora to gather Fedora packages (dnf runs
anywhere), but it is simplest when it is.

### Cargo crates

Every `.crate` cargo has ever downloaded on a machine is in its registry
cache — that is the whole mirror for the projects you build:

```sh
cargo fetch                                        # in each project, online
depot ingest crate ~/.cargo/registry/cache/*/       # every cached .crate
```

To include a project's full dependency graph for **all** targets (a crate
that only builds on musl is still fetched):

```sh
cargo fetch --target aarch64-unknown-linux-musl --target x86_64-unknown-linux-gnu
```

For a single crate: `curl -O https://static.crates.io/crates/serde/serde-1.0.219.crate`.

A depot gives the sparse protocol (`config.json` + `index/<pa>/<th>/<name>`),
which cargo ≥ 1.70 uses by default. Yanked versions are marked exactly as
crates.io marks them; the file stays downloadable (`depot yank`).

### Fedora RPMs

For a **full system reload** you want everything an installed system needs,
plus the packages you will add. On a box that looks like the target:

```sh
# 1. the list of what is installed
dnf repoquery --installed --qf '%{name}\n' | sort -u > pkgs.txt

# 2. download those with every dependency resolved, for the target arch
dnf download --resolve --alldeps --arch x86_64,noarch --destdir ./rpms $(cat pkgs.txt)

# 3. the signing key dnf will verify against
cp /etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-42-primary ./

# 4. ingest into a repo named for what it is
depot ingest rpm --repo fedora-42-x86_64 ./rpms
depot rpm gpg-key --repo fedora-42-x86_64 RPM-GPG-KEY-fedora-42-primary
depot rpm describe --repo fedora-42-x86_64 "Fedora 42 + updates, workstation set, 2026-09"
```

To mirror **an entire repository** instead (tens of GB; do it once, then
weekly deltas ride in packs):

```sh
dnf reposync --repo=fedora --repo=updates --arch=x86_64 --download-metadata \
             --newest-only -p /srv/reposync
depot ingest rpm --repo fedora-42-x86_64 /srv/reposync/fedora/Packages/*/ /srv/reposync/updates/Packages/*/
```

Keep `fedora` and `updates` in **one** depot repo: dnf picks the newest NEVRA
across the repos it sees, and depot's repodata already lists every version.
A separate repo id is for a different distribution/release/arch, or for a
COPR/third-party set you want to enable independently.

The repodata depot writes (`primary`, `filelists`, `other`) is byte-for-byte
what `createrepo_c` writes for the same packages — verified in the test
suite against createrepo_c's output. What it does **not** write:
`comps.xml` (group definitions: "@workstation-product-environment"),
`updateinfo.xml` (advisories, `dnf updateinfo`), and `modules.yaml`. dnf
works fully without them; `dnf group install` and Anaconda's environment
picker do not (see §8).

### Nix binary cache

On a machine with nix, for the closures you need:

```sh
# sign first, or the offline nix will refuse them (require-sigs is on by default)
nix key generate-secret --key-name depot-1 > ~/.config/nix/depot-1.sec
nix key convert-secret-to-public < ~/.config/nix/depot-1.sec        # → nix.conf on clients
nix store sign --key-file ~/.config/nix/depot-1.sec --recursive nixpkgs#hello

nix copy --to file:///tmp/nixcache nixpkgs#hello                     # a closure
nix copy --to file:///tmp/nixcache $(nix path-info -r /run/current-system)  # a whole NixOS system
depot ingest nix /tmp/nixcache
```

`nix copy --to file://` writes exactly a binary cache (narinfos + `nar/`);
depot lifts it in, verifying each NAR against its narinfo. Paths signed by
cache.nixos.org keep that signature and are trusted by every default nix.conf.

### Rust toolchains

`maintenance/pack-rust.sh` (in this repo) downloads and verifies the
standalone component tarballs for a version and a set of triples:

```sh
maintenance/pack-rust.sh 1.96.0 x86_64-unknown-linux-gnu aarch64-unknown-linux-musl aarch64-unknown-linux-gnu
depot ingest toolchain rust-install/
```

Or single files from `https://static.rust-lang.org/dist/<component>-<version>-<triple>.tar.xz`
(`rustc`, `cargo`, `rust-std`; add `rust-std-<version>-<other-triple>` for
cross-compiling, `clippy`, `rustfmt`, `rust-src` as needed).

---

## 4. Sync

### Sneakernet

```sh
# offline depot
depot have --out have.txt                          # one hash per line; small

# online depot (have.txt carried over)
depot pack --out 2026-09-12.depot --have have.txt --all
#   or a subset: --crates --rpm fedora-42-x86_64 --nix --toolchains

# offline depot, first time only: trust the online depot's key
depot trust online <hex from `depot status` on the online side, or the keys/depot.pub file>
depot unpack 2026-09-12.depot
```

Without `--have`, the pack contains everything selected — fine for the first
fill, wasteful after. `depot inspect FILE` shows what a pack carries before
you unpack it.

### Over HTTP

If the offline depot can reach the online one (e.g. through a phone
hotspot for an hour), pull a pack directly:

```sh
depot have | curl -sS --data-binary @- "http://online.lan:8000/api/pack?select=all" -o sync.depot
depot unpack sync.depot
```

Or push into a depot that has an admin token:

```sh
curl -X PUT -H "Authorization: Bearer $TOKEN" --data-binary @sync.depot http://depot.lan:8000/api/packs
```

### Unsigned packs

`depot pack --unsigned` / `depot unpack --allow-unsigned` exist for a depot
that has no key yet or a stick you carried yourself. The signature is what
lets you unpack a stick someone else handed you.

---

## 5. Configuring devices (the offline side)

The **Clients** tab of the UI generates the exact files for the boxes you
tick, with the depot's URL filled in, and a one-liner:

```sh
curl -fsSL "http://depot.lan:8000/bootstrap.sh?crates=1&rpm=fedora-42-x86_64&toolchain=1" -o bootstrap.sh
DRY=1 sh bootstrap.sh      # prints what it would write
sudo sh bootstrap.sh
```

Or from the CLI on the depot: `depot client-config --server http://depot.lan:8000 --all`.
What each one does:

### cargo — `~/.cargo/config.toml`

```toml
[source.crates-io]
replace-with = "depot"
[source.depot]
registry = "sparse+http://depot.lan:8000/crates/index/"
```

This is **source replacement**: cargo still names the registry `crates-io`
in `Cargo.lock`, so lock files made online and offline are identical, but
fetches from the depot. Do not set `net.offline = true` — that forbids all
fetching, including from the depot.

If a build fails with `failed to get 'foo' as a dependency`, the depot lacks
that crate (or that version): `depot ingest crate` it on the online side.

### dnf — `/etc/yum.repos.d/depot-<repo>.repo`

```ini
[depot-fedora-42-x86_64]
name=depot mirror: fedora-42-x86_64
baseurl=http://depot.lan:8000/rpm/fedora-42-x86_64/
enabled=1
gpgcheck=1
gpgkey=http://depot.lan:8000/rpm/fedora-42-x86_64/RPM-GPG-KEY
metadata_expire=1h
skip_if_unavailable=False
```

Then stop dnf from trying the internet:

```sh
sudo dnf config-manager setopt fedora.enabled=0 updates.enabled=0 fedora-cisco-openh264.enabled=0
# or per command:  dnf --disablerepo='*' --enablerepo='depot-*' install foo
sudo dnf makecache
```

`gpgcheck=1` works because the packages are Fedora's, signed by Fedora; depot
serves the same key you copied from `/etc/pki/rpm-gpg/`. If no key is
attached the generated file says `gpgcheck=0` and explains why.

### nix — `/etc/nix/nix.conf`

```
substituters = http://depot.lan:8000/nix https://cache.nixos.org
trusted-public-keys = depot-1:<public key> cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=
```

The depot is tried first (its `Priority: 30` beats cache.nixos.org's 40);
the public cache stays listed for boxes that can reach it. On NixOS use
`nix.settings.substituters` / `nix.settings.trusted-public-keys`.

### Rust toolchain

```sh
curl -fsSL http://depot.lan:8000/toolchains/install.sh -o i.sh
sh i.sh 1.96.0 "$HOME/.local"                       # or /usr/local as root
```

The script detects the host triple (glibc vs musl via `ldd`), verifies each
tarball's sha256 against `index.json`, and runs the components' own
`install.sh`. No rustup. To cross-compile, install `rust-std` for the other
triple into the same prefix:

```sh
COMPONENTS=rust-std TRIPLE=aarch64-unknown-linux-musl sh i.sh 1.96.0 "$HOME/.local"
```

### Full system reload from the depot

A Fedora **netinstall** ISO (or a kickstart) can install straight from the
depot repo:

```
inst.repo=http://depot.lan:8000/rpm/fedora-42-x86_64/
```

Two caveats, both from what depot does not write (§3): without `comps.xml`
Anaconda cannot offer environments/groups, so use a **kickstart** with an
explicit `%packages` list; and the repo must contain the whole install set
(`dnf download --resolve` of the target's package list gets you there).
After first boot, the `.repo` file above makes `dnf` keep working.

For postmarketOS/Alpine devices the equivalent is an `apk` repository —
not in scope for depot today (a fifth projection would follow the same
pattern: `APKINDEX.tar.gz` derived from the `.apk` control segments).

---

## 6. Operating

| task | command |
|------|---------|
| what is here | `depot status`, or the Overview tab |
| free space check | `depot status` reports objects + bytes; `du -sh <root>/objects` agrees |
| re-hash everything | `depot verify` (also the Sync tab's button) — refuses to lie about a bad disk |
| regenerate pub/ | `depot rebuild` (safe any time; atomic) |
| reclaim space | `depot gc` lists unreferenced objects, `depot gc --delete` removes them |
| enable HTTP writes | `depot admin-token --new`; delete `keys/admin.token` to disable |
| rotate the token | delete the file, run `--new` again; takes effect immediately |
| back up | `rsync -a <root>/{catalog.json,objects,keys,trust,depot.toml} …` (pub/ and tmp/ are derived) |
| logs | `journalctl -u depot` — one line per request: method, path, status, bytes, ms |
| learn / teach | `depot tutorial` lists the guides; `depot tutorial server-setup` prints one; the UI shows them a step at a time, with a live check per step where the depot can verify itself |
| diagrams | `depot niftyplot FILE` (or `-` for stdin) renders a `.niftyplot` diagram to SVG on stdout; the same engine draws the fenced diagrams in tutorials |

Nothing in depot needs the clock to be right (offline boxes often have it
wrong): timestamps are informational, signatures have no expiry, and dnf's
`metadata_expire` is relative.

---

## 7. HTTP API

Reads are open; writes need `Authorization: Bearer <admin token>`.

| method | path | body / query | result |
|--------|------|--------------|--------|
| GET | `/api/status` | | counts, sizes, keys, whether writes are enabled |
| GET | `/api/crates?q=` · `/api/crates/<name>` | | crate list / versions |
| PUT | `/api/crates` | a `.crate` | ingest |
| POST | `/api/crates/<name>/<vers>/yank` · `/unyank` | | mark |
| GET | `/api/rpm` · `/api/rpm/<repo>?q=` | | repos / packages |
| PUT | `/api/rpm/<repo>` | an `.rpm` | ingest + repodata rebuild |
| GET | `/api/nix` · `/api/toolchains` | | lists |
| PUT | `/api/toolchains` | tarball, `X-Filename:` | ingest |
| GET | `/api/have` | | hash list, one per line |
| POST | `/api/pack?select=all\|crates,nix,toolchains,rpm:<id>,rpm:*[&unsigned=1]` | have-list | a pack (streamed) |
| PUT | `/api/packs[?allow_unsigned=1]` | a pack | unpack |
| GET | `/api/client-config?crates=1&rpm=<id>&nix=1&toolchain=1` or `?all=1` | | snippets as JSON |
| GET | `/api/verify` | | `{"corrupt":[…]}` |
| GET | `/api/tutorials` · `/api/tutorials/<slug>.md` | | tutorials as steps (JSON, each with its live check outcome) / raw markdown |
| POST | `/api/niftyplot` | diagram source | SVG (`image/svg+xml`) |
| GET | `/bootstrap.sh?…` · `/crates/index/config.json` · `/toolchains/install.sh` | | generated per request |

Everything under `/crates/`, `/rpm/`, `/nix/`, `/toolchains/` is a static
file with `ETag`, `Range` and `HEAD` support; directories list.

---

## 8. Configuration — `depot.toml`

```toml
[server]
bind = "0.0.0.0:8000"          # LAN-only by firewall, not by bind address
admin_token_file = "keys/admin.token"
workers = 8                    # threads; dnf opens several connections
log = true
public_url = ""                # e.g. "http://depot.lan:8000"; "" = derive from Host

[rpm]
compression = "gz"             # "zst" needs a build with --features zstd; dnf ≥ 4.4
repodata = "hashed"            # <sha256>-primary.xml.gz — immutable, cache-safe

[crates]
registry_name = "depot"        # the [source.<name>] in generated cargo config

[nix]
store_dir = "/nix/store"
priority = 30                  # lower wins; cache.nixos.org is 40
```

---

## 9. Troubleshooting

**cargo: `failed to download from http://depot…/crates/dl/...` (404)** — the
index knows the version but the object is gone: `depot verify` then `depot
rebuild`. If the version is genuinely missing, the online side never
ingested it.

**cargo: `sparse registries require cargo 1.68+`** — old toolchain; install
one from the depot's toolchains first (the installer only needs `curl`).

**dnf: `Cannot download repomd.xml`** — firewall (port 8000), wrong host
name, or `skip_if_unavailable=False` doing its job. `curl -I
http://depot.lan:8000/rpm/<repo>/repodata/repomd.xml` from the client.

**dnf: `GPG check FAILED`** — the attached key does not match the packages
(a Fedora 41 key on 42 packages). Attach the right one; `rpm -qi
<package>` prints the key ID it was signed with.

**dnf: `nothing provides @workstation-product-environment`** — groups need
`comps.xml`, which depot does not write. Install by package list.

**nix: `substituter … is not signed by any trusted key`** — add the signing
key to `trusted-public-keys`, or sign the cache on the online side (§3).

**nix: `warning: … does not have a NAR file`** — the narinfo made it into a
pack but the NAR did not: `depot verify`; if the NAR is missing, re-pack
with a fresh `have.txt`.

**`unpack`: `signature matches none of the trusted key(s)`** — the online
side rotated its key, or you trusted the wrong file. `depot status` on the
producer shows the public key; compare with `trust/*.pub`.

**SELinux: `depot serve` cannot bind 8000** — only if you confined it in a
custom policy; stock Fedora runs it unconfined. `semanage port -l | grep
8000` shows the label (`soundd_port_t`); `semanage port -a -t http_port_t
-p tcp 8000` relabels it if you did confine it.

**Phone as the depot** — it works (musl build, `~/.local/share/depot`), but
`workers = 4` and expect hashing a 1 GB pack to take a minute. Serving is
just `sendfile`-speed.

---

## 10. Not yet

- **rustup against the depot.** rustup needs the channel manifest
  (`dist/channel-rust-stable.toml` + `.sha256`) and the same tarballs under
  `dist/<date>/`. Mirror those two files alongside the tarballs and point
  `RUSTUP_DIST_SERVER=http://depot.lan:8000/toolchains` at it — the layout
  is one flag away; depot does not fetch the manifest today.
- `comps.xml` / `updateinfo.xml` for dnf groups and advisories.
- HTTPS. The transport crate (`depot-httpd`) is the seam; a TLS listener
  slots in without touching routing. On a LAN you own, the artifact
  signatures are what protect you, not the pipe (§DECISIONS 3).
- Alpine/postmarketOS `apk` repositories.
