# Paperbound — Multiplayer, Options & Deck Builder

Three features shipped in this build: **LAN multiplayer**, an **expanded deck
builder**, and **graphics presets** (Low + Photorealistic).

---

## 1. LAN Multiplayer (`netplay.py` + `game.py`)

### How to play a LAN match
1. Both players run `python game.py` on the same network.
2. **Host**: Main menu → **Multiplayer (LAN)** → **Host on port 7777**. The page
   shows your LAN address(es).
3. **Client**: **Multiplayer (LAN)** → click the address field, type the host's
   IP, click **Join**.
4. The match starts automatically the moment the host sees the client. Host is
   team 0 (bottom), client is team 1 (top). A `◈ LAN · HOST/CLIENT` tag shows
   in the battle HUD.

### How it works — delayed-input lockstep
The two machines each run the **same deterministic simulation** at 20 TPS. The
only world-changing input is a card deployment, so that's all we send over the
wire. Each deploy is stamped with the tick it should apply on (`current + 4`,
~200 ms ahead), so the same input fires on the same tick on both machines. No
full-state streaming, no rollback — just a few bytes per card played.

Key pieces:
- `netplay.Host` / `netplay.Client` — thin UDP peers, JSON datagrams, a
  background listen thread, and a thread-safe `poll()` inbox.
- `Game(netmode="host"|"client", seed=…)` — seeds `random` so both sims match;
  disables the local AI and the Angler boss.
- `Game.net_local_play()` — spends ink locally, schedules the deploy
  `INPUT_DELAY` ticks ahead, and broadcasts it.
- `Game.net_schedule()` / `_apply_tick_inputs()` — the tick-stamped input queue
  applied identically on both ends.

### Verified
- **Determinism test**: two seeded sims fed identical inputs produce
  byte-identical unit rosters after 60 ticks. ✅
- **Live socket test**: real Host+Client over UDP exchange seed + a deploy that
  lands on the same tick on both sims. ✅

### Protocol (all JSON, one datagram each)
```
{"t":"hello","name":...}                                  client → host
{"t":"start","seed":int,"arena":str,"tick":0}             host → client
{"t":"deploy","tick":int,"key":str,"x":..,"y":..,"team":..} either way
```

---

## 2. Expanded Deck Builder

The deck builder is now a full three-column workbench:

- **Left rail** — filter by *All / Troops / Buildings / Flying*, plus a
  *Sort: cost/name* toggle.
- **Middle** — the filtered, sorted card grid. Hover a card to inspect it;
  locked cards still show as torn-out pages.
- **Right panel** — live card detail: big preview, Cost / Health / Damage /
  Count / Type stats, and the sigil chip with its wrapped description.
- **Bottom** — your 8 deck slots (click one to remove it), an **average cost**
  read-out in the header, and a little **cost-curve histogram**.

---

## 3. Graphics Presets (Options page)

Main menu → **Options** → pick a rendering preset. Choice takes effect instantly
and rebuilds the board texture.

| Preset | What you get | Cost |
|--------|--------------|------|
| **Low** (default) | The original fast look: flat fills, single-layer shadows, 24-step lightmap. | cheap |
| **Photorealistic** | 56-step lightmap with flame bloom cores, **3-layer soft-penumbra shadows** + contact AO under units, a full-screen **bloom** pass, scrolling **film grain**, and a warm cinematic vignette. | heavier |

All the extra work lives behind `gfx_hi()` checks in `build_lightmap()`,
`cast_shadow()`, and the new `apply_postfx()` — so Low mode runs exactly as
before.

### Screenshots
- `paperbound_gfx_compare.png` — low vs photorealistic, side by side
- `paperbound_options.png` — the preset picker
- `paperbound_multiplayer.png` — the LAN lobby
- `paperbound_deckbuilder.png` — the expanded builder

---

## Files
- `game.py` — main game (menus, battle, render, net integration)
- `netplay.py` — standalone UDP lockstep networking (no pygame dep; self-tests
  with `python netplay.py`)

---

## LAN on phones (postmarketOS / Alpine / Wayland) & safety

Recent additions make LAN play work well on mobile Linux and safer on public
networks at LAN parties.

### Typing on a phone (virtual keyboard)
The Join screen works three ways so text entry never blocks you:
- **On-screen numpad** — a built-in digit/dot/backspace pad for entering an IP.
  This always works, even if no system keyboard is installed. Just tap.
- **Virtual keyboard (Wayland text-input v1/v2/v3)** — tapping the IP or name
  field calls `SDL_StartTextInput`, which asks the compositor (phosh/squeekboard,
  maliit, etc.) to raise the on-screen keyboard, and reads `TEXTINPUT` events so
  composed characters arrive correctly. SDL hints (`SDL_IME_SHOW_UI`,
  `SDL_HINT_ENABLE_SCREEN_KEYBOARD`) are set at startup to enable this.
- **Hardware keyboard** — plug one in and type normally; `KEYDOWN` is handled as
  a fallback.

### Firewall (can't connect?)
Paperbound uses **UDP port 7777**. Many phone distros block inbound UDP by
default. If a friend can't reach your hosted table, allow the port. On
postmarketOS / Alpine (nftables):

```
sudo nft add rule inet filter input udp dport 7777 accept
```

or with iptables:

```
sudo iptables -A INPUT -p udp --dport 7777 -j ACCEPT
```

The Join screen shows your LAN address(es) and reminds you of this. (The game
can't open the firewall itself — that needs root — so it gives you the exact
command instead.)

### Usernames & trust (public networks)
- Set a **username** in the field at the top of the Join screen; it's saved
  between sessions.
- When you **host**, joiners don't auto-connect. Each appears as a **join
  request** (name + IP) in the waiting room with **Approve** / **Deny** buttons,
  so a stranger on a café network can't drop into your game uninvited.
- **Approve** remembers that username as **trusted**, so friends who rejoin later
  are admitted automatically. The trusted list is persisted.
- A denied peer is told and blocked; a waiting joiner sees "waiting for the host
  to approve you".

---

## Connection channels & firewall config (advanced)

Paperbound can carry its LAN games over more than one channel, chosen on the Join
screen with the **Channel** picker. Pick whichever your network allows:

| Channel | Port (default) | Notes |
|---------|----------------|-------|
| **HTTP** (recommended) | TCP **8677** | A tiny HTTP relay the host runs. TCP + a well-known protocol that firewalls and proxies are comfortable with; advertises itself over **mDNS/Avahi** so it's discoverable even where UDP broadcast is blocked. Pure stdlib, no extra packages. |
| **UDP** | UDP **7777** | The original low-latency datagram channel. Fastest, but blocked by default on many phone distros and public networks. |

Both channels run the **identical deterministic lockstep sim** and the same
username/trust approval flow — only the wire transport differs.

### Discovery
"Scan LAN" tries, in order: **mDNS/Avahi** (if the optional `zeroconf` Python
package is installed — `pip install zeroconf`), then the built-in **UDP
broadcast** ping. If both are filtered, just type the host's IP and Join — over
HTTP the client connects straight to the address.

### Opening the port (host side)

**nftables (Alpine / postmarketOS default):**
```
# HTTP channel
sudo nft add rule inet filter input tcp dport 8677 accept
# or UDP channel
sudo nft add rule inet filter input udp dport 7777 accept
# mDNS discovery (usually already allowed)
sudo nft add rule inet filter input udp dport 5353 accept
```

**iptables:**
```
sudo iptables -A INPUT -p tcp --dport 8677 -j ACCEPT
sudo iptables -A INPUT -p udp --dport 7777 -j ACCEPT
```

**Persist it (Alpine):** put the rule in `/etc/nftables.nft` (or
`/etc/iptables/rules-save`) and enable the service:
```
rc-update add nftables
rc-service nftables restart
```

**systemd socket unit** (if you want the port reserved/trusted at the init level
on a systemd phone). Create `~/.config/systemd/user/paperbound.socket`:
```
[Socket]
ListenStream=8677
Accept=no

[Install]
WantedBy=sockets.target
```
This declares the port to the system so it's a known, allowed service. (The game
still opens its own listener; the unit is a way to make the port an
administratively-blessed part of the config.)

### Bluetooth
If BlueZ is present the game detects an available Bluetooth stack. Full RFCOMM
transport isn't bundled (it needs BlueZ/PyBluez, which aren't in the standard
library), but you can bridge two paired phones with a PAN/`bnep` network and then
use the HTTP channel over that link. Pair the phones in your OS settings first.

### Security note
The trust/approval + usernames stop *uninvited* players from joining, but LAN
traffic itself is unauthenticated. On a hostile network, tunnel over a VPN
(WireGuard) and host on the VPN address.
