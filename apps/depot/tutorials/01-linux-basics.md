# Linux basics for running a server on your LAN

What you need to know to run depot — or any small service — on a Linux
box, with the commands to try as you go. Each step ends with what you
should see. Nothing here is depot-specific; depot is just the example.

## Where files live

Linux puts things in fixed places, and the places mean something. The
short version:

| path | what | depot's use |
|------|------|-------------|
| `/usr` | the operating system: programs, libraries. Read-only in practice. | `/usr/local/bin/depot` (local = not from a package) |
| `/etc` | configuration, one file or directory per program | `/etc/systemd/system/depot.service`, `/etc/yum.repos.d/*.repo` |
| `/var/lib` | state a service keeps between reboots | `/var/lib/depot` |
| `/var/cache` | data a service could regenerate — safe to delete | dnf's cache |
| `/var/log` | logs (on systemd boxes, the journal lives here) | |
| `/tmp` | scratch, cleared at boot | uploads in flight |
| `~/.local/share` | a user's own data (the XDG data directory) | `~/.local/share/depot` when not root |
| `~/.config` | a user's own configuration | `~/.cargo/config.toml` is the exception: cargo predates XDG |

Try it:

```sh
ls /var/lib | head            # every service that keeps state has a directory here
ls ~/.local/share | head      # the same idea, for you
```

You should see directories named after programs. When a program asks
"where should I put this?", the answer is almost always one of the rows
above.

## Users, ownership and permission bits

Every file has an owner, a group, and nine permission bits shown by
`ls -l` as three triplets: owner, group, everyone — each `r` (read),
`w` (write), `x` (execute, or "enter" for a directory). The same thing
as a number: `r`=4, `w`=2, `x`=1, so `rw-r--r--` is `644` and `rw-------`
is `600`.

```sh
id                                    # who you are: uid, gid, groups
touch example && ls -l example        # -rw-r--r--  644, the usual default
chmod 600 example && ls -l example    # -rw-------  only you can read it
chmod 444 example && echo hi > example
```

You should see `Permission denied` on the last line — a file you own but
made read-only refuses even you. That is the trick depot uses: every
object in the store is `0444`, so a stray `>` cannot corrupt it. Secrets
(`keys/depot.key`, `keys/admin.token`) are `0600`: nobody else on the box
can read them.

One exception to remember: **root ignores permission bits.** As root the
`echo` above succeeds. That is why services should not run as root
(next steps).

## Hardlinks: two names, one file

A file on disk is an *inode* (the data plus its metadata); a name in a
directory just points at one. `ln` adds a second name. The file exists
until the last name is removed, and both names see the same bytes.

```niftyplot
%kind: graph
%title: "Two names, one inode"
A as note: "objects/ab/ab12…"
B as note: "pub/rpm/f42/Packages/h/hello.rpm"
I as cylinder: "inode 4711 — the bytes"
A → I
B → I
```

```sh
echo data > a
ln a b                    # hardlink: b is another name for a's inode
ls -li a b                # same inode number, link count 2
rm a && cat b             # still there
ln -s b c && ls -l c      # symlink: c is a *path* that says "look at b"
```

You should see identical inode numbers for `a` and `b`, and `c -> b` for
the symlink. depot's `pub/` tree is hardlinks into `objects/`: the same
RPM in two repos costs one copy, and `pub/` can be deleted and rebuilt
without touching the data. Symlinks would work too, but every request
would pay an extra lookup and the server would have to police where they
point.

## Processes and ports

A server is a process that *listens* on a port. Ports below 1024 need
root (or a capability); anything above is free for any user — which is
why depot uses 8000 and the service runs without privileges.

```sh
ss -ltnp                            # every listening TCP port and the process behind it
ss -ltnp | grep :8000               # is depot up?
curl -sI http://127.0.0.1:8000/     # talk to it: -I asks for headers only
```

You should see `LISTEN 0.0.0.0:8000` with `depot` in the process column,
and `HTTP/1.1 200 OK` from curl. If `curl` says `Connection refused`, the
process is not listening — the port is wrong or the service is down.
Nothing about the firewall applies to `127.0.0.1`; that comes later.

## systemd: services, units, and the journal

systemd starts things at boot and keeps them running. A *unit file*
describes one service; `systemctl` controls it; `journalctl` shows what it
printed. The service's stderr *is* its log — no log files to rotate.

```sh
systemctl cat depot            # the unit file, as systemd sees it
systemctl status depot         # running? since when? last log lines
sudo systemctl enable --now depot    # start now AND at every boot
journalctl -u depot -f         # follow the log (Ctrl-C to stop)
journalctl -u depot --since "1 hour ago"
```

After editing a unit file: `sudo systemctl daemon-reload` before
`restart`, or systemd keeps using the old one. You should see `Active:
active (running)` and, in the journal, one line per request.

## Finding the box on the network

Every machine has an address per interface; the LAN one is what clients
use. A *name* is a convenience layered on top: `/etc/hosts` on each
client is the zero-infrastructure way, DNS on the router the tidy way.

```sh
ip -4 addr                                # your addresses; look for the LAN interface
ip route | head -1                        # default via <router> — the LAN you are on
echo "192.168.1.20 depot.lan" | sudo tee -a /etc/hosts    # on a client
getent hosts depot.lan                    # does the name resolve?
ping -c 2 depot.lan
```

You should see the address you added. `.local` names (`depot.local`) come
from mDNS and work only where avahi runs and only within one subnet;
`.lan` with a real entry works everywhere.

## The firewall

Fedora ships firewalld: interfaces belong to *zones*, and a zone lists
what is allowed in. A new service's port is closed until you open it — a
listening process and a closed port look identical from another machine
(`curl` hangs, then times out).

```sh
sudo firewall-cmd --get-active-zones                    # which zone is your LAN interface in?
sudo firewall-cmd --permanent --add-port=8000/tcp       # open it in the default zone
sudo firewall-cmd --reload
sudo firewall-cmd --list-ports                          # 8000/tcp should be listed
```

Phones and SBC images often ship no firewall at all: `sudo nft list
ruleset` printing nothing means nothing is filtered. From another machine
`curl -sI http://depot.lan:8000/` should now answer; a hang means the
port is still closed or you opened it in the wrong zone.

## Checksums and signatures — two different promises

A **hash** (`sha256`) is a fingerprint of the bytes: same bytes, same
hash; one bit flipped, a different hash. It proves the file is *intact*.
It does not say who made it — anyone can hash anything.

```sh
sha256sum depot.service          # fingerprint
sha256sum depot.service > sums && sha256sum -c sums     # verify later
```

A **signature** is a hash *plus* a proof it was made by someone holding a
private key; you check it with their public key. It proves *who*. Fedora
signs RPMs (dnf checks with `gpgcheck=1`); nix signs store paths; depot
signs packs with an ed25519 key so the offline side knows the stick came
from the online depot and not from someone with the same USB brand.

You should see `depot.service: OK` from `-c`. depot names every object by
its sha256 — the name *is* the checksum, which is why `depot verify` can
find a bad disk sector without any extra bookkeeping.

## What a package manager wants from a server

dnf, cargo, nix and apk are different programs with one thing in common:
what they fetch is **static files** — an index that says what exists and
where, and the packages themselves. No server logic, no database, no
login. That is why one plain file server can stand in for crates.io,
Fedora's mirrors and cache.nixos.org at once, and why a USB stick can
carry a whole repository.

```niftyplot
%kind: graph
%title: "What each client reads first"
plane clients {
    cargo
    dnf
    nix
    sh: "install.sh"
}
plane depot {
    C: "/crates/index/config.json"
    D: "/rpm/<repo>/repodata/repomd.xml"
    N: "/nix/nix-cache-info"
    T: "/toolchains/index.json"
}
cargo → C
dnf → D
nix → N
sh → T
```

```sh
curl -s http://depot.lan:8000/crates/index/config.json          # cargo's "where do I download from"
curl -s http://depot.lan:8000/rpm/fedora-42-x86_64/repodata/repomd.xml | head -5
curl -s http://depot.lan:8000/nix/nix-cache-info
```

You should see JSON, XML, and a three-line text file. Each is the entry
point its client reads first; everything else is fetched by the paths
those files name.

## Reading an error

When something fails, find *which side* failed before changing anything:

```sh
curl -v http://depot.lan:8000/crates/index/config.json 2>&1 | grep -E "^[<>] |Failed|refused"
journalctl -u depot -n 20            # did the request even arrive? what status did it get?
```

The status codes you will meet: `200` fine · `304` unchanged, use your
cache · `206` partial (a byte range, normal for dnf) · `401` you sent no
or a wrong token · `403` refused on purpose · `404` the depot has no such
path — usually a missing package, not a server bug · `connection refused`
not listening · a hang: firewall.

If the journal shows the request with a `200` and the client still
complains, the problem is on the client (its config, its cache:
`dnf clean metadata`, `rm -rf ~/.cargo/registry/index/*depot*`). If the
request never shows up, it is the network or the name.
