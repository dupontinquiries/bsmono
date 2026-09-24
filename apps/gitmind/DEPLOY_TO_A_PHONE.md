# Getting a phone into the fleet from gitmind

From a laptop running gitmind-egui to a phone that was imaged an hour ago. What each
transport can do, the order things happen in, and what has and has not been tried on a
real phone. Deploying an app once the phone is enrolled is
[`apps/tideshell/deploy/GITMIND-DEPLOY.md`](../tideshell/deploy/GITMIND-DEPLOY.md).

## The transports

| transport | what it can do | where it lives |
|---|---|---|
| **SSH** | everything: every deploy, build and enrol step runs over it | `cockpit::deploy` |
| **USB networking** | the same as SSH, over the cable: postmarketOS puts the phone on `172.16.42.x` with the laptop on the same link; discovery scans the `usb0` tether | `deploy::local_subnet*`, the Fleet scan |
| **Bluetooth** | pushes a FILE (OBEX Object Push: a bundle, a script); it cannot run anything | `cockpit::bttransfer` |
| **A browser + curl** | first contact with a phone that has no sshd yet (below) | `cockpit::join_server` |

A fresh postmarketOS image has `user` / `147147` and **no sshd running**, so SSH (over Wi-Fi
or USB) cannot be the first step. The browser join is.

## 1. First contact: "join a new phone"

On the laptop: Fleet page → "no sshd yet?" → choose the address the phone can reach (every
local IPv4 with its interface is listed; the USB link is preferred when present, because the
default route is the Wi-Fi one on a tethered laptop) → **join a new phone**. It listens on
port 8099 (any free port if that is taken).

On the phone:
1. open `http://<laptop address>:<port>/` in the browser;
2. type the **6-digit code** the laptop shows (three wrong codes close the session);
3. tap the **number** the laptop shows, out of the three the phone offers (a wrong tap closes
   the session);
4. the page gives one command -- run it in a terminal on the phone. It fetches the join
   script with a single-use token and checks its SHA-256 before running it. **Compare the
   fingerprint on the phone's page with the one on the laptop first**;
5. the script starts sshd (now and at boot), creates the fleet account (default `gitmind`,
   no password, key-only), installs the laptop's SSH key, and reports hostname, host key and
   architecture back. The laptop shows them with the phone's address as seen on that
   connection -- the SSH target to use next.

Codes expire after 10 minutes. Needs `~/.ssh/id_ed25519.pub` (or ecdsa/rsa) on the laptop;
without one the panel says so instead of starting.

**What the steps protect against, plainly** (the module doc has the reasoning): this is plain
HTTP on a local network.
- The **code** keeps anyone else on the LAN from getting the script -- a one-time code in the
  role of a login and password, with nothing stored to leak.
- The **number match** confirms the page the phone shows belongs to the session on the
  laptop's screen.
- The **fingerprint** is what catches someone relaying the exchange and altering the script
  -- **only if you compare it**. Without TLS that comparison is the limit of what this can
  guarantee.
- The join script does **not** remove `user`'s password: `user` is the way back in when a join
  goes wrong. The fleet account never gets one.

## 2. Enrolment

With sshd up, the Fleet page's device list: scan, trust the host key (compare it with the one
the join reported), install the key for the fleet user. The roster pins the host key; a
reflash shows up as a changed key.

## 3. depot credentials (optional)

To let gitmind write to a depot without copying its admin token around, pair once:

```
gitmind depot config depot.lan:8000 ssh=admin@depot.lan   # where it is; ssh= enables approval over ssh
gitmind depot pair                                        # prints a request id and a 6-digit code
gitmind depot approve                                     # runs `depot pair approve` on the depot host over ssh
#   or: gitmind depot approve --token <admin token>       # over HTTP, token typed once, never stored
gitmind depot status                                      # "paired as gitmind-<host>"
gitmind depot put-crate some.crate                        # a signed upload; the stored sha256 is checked
```

On the depot host the same request shows in `depot pair list` with the same code: approve only
if they match. `depot pair revoke <name>` ends it. Protocol and scopes:
[`apps/depot/MANUAL.md`](../depot/MANUAL.md) section 7.

## What has been tried where

| step | verified | not verified |
|---|---|---|
| join server | the whole flow in tests and over a real socket; the phone command run with real `curl` + `sha256sum` (GNU coreutils), a tampered hash refused | on a phone; **busybox `sha256sum -c -`** (postmarketOS) reading the check from stdin |
| depot pairing | a live `depot serve` on localhost: pair, wrong code refused, approve, signed upload with hash check, revoke, approve with an admin token | approval over ssh (`gitmind depot approve` without `--token`) |
| USB networking | discovery scanning `usb0` (tests) | a join over the USB link |
