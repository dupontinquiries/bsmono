# The managed fleet user

The Fleet cockpit connects as a dedicated **fleet user** (default `gitmind`) rather
than your personal login. This is standard service-account practice, and here it
also makes the earlier key hardening sharper.

## Why

- **Isolation** — fleet automation can't touch your personal account's files,
  history, or dotfiles. A compromised fleet key gets a limited account, not you.
- **Least privilege** — combined with the restricted key (`restrict,from="<lan/24>"`,
  no agent/port forwarding), the fleet key is a narrow, LAN-only, no-tunnel account.
- **No surprise default** — the default isn't "whoever ran whoami"; it's an explicit,
  GUI-managed name you can see and change (persisted under `~/.config/gitmind/`).

## How it's managed

- `fleet_user()` reads the configured name (default `gitmind`); `set_fleet_user`
  persists it (the "save" button). The username is validated before it ever reaches
  a shell (`valid_username`: lowercase-start, `[a-z0-9_-]`, ≤32) so it can't carry an
  injection.
- **Provisioning** (`create fleet user`, one time per device): over ssh as your
  sudo-capable account, the GUI creates the user portably (`useradd` on Fedora /
  busybox `adduser` on Alpine/postmarketOS), sets up its `~/.ssh`, and installs the
  restricted key. The script is base64'd for transport (no shell-quoting hazards) and
  run under `sudo -S` with the password fed on stdin. Nothing is written to disk.

## Bootstrap order

1. Provision the fleet user on a device (needs your sudo password once), **or**
   install your key for an account that already exists ("install my key").
2. After that, connect as the fleet user — passwordless, restricted, LAN-scoped.

The `cross build`, `useradd`/`adduser`, and `sudo` all run on the real devices; the
sandbox verifies the config handling, username validation, base64, and the script /
command construction only.

## Note

The Network tab's advertised `ssh://` provider still uses your login name; making it
advertise the fleet user too is a small, sensible follow-up for full consistency.
