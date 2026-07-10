"""
Paperbound LAN networking — lockstep deployment sync over UDP.

Design (kept deliberately simple and robust for a card-battler):
  • Both peers run the *same* deterministic simulation at 20 TPS.
  • The only inputs that change the world are card deployments, so we only need
    to exchange those. Each deploy is stamped with the tick it should apply on.
  • We use delayed-input lockstep: an input made on tick T is scheduled to run
    on tick T+DELAY on both machines (default 4 ticks ≈ 200ms), which hides LAN
    latency and keeps both sims identical without rollback.
  • Host is authoritative for the RNG seed and match start.

Message shapes (all JSON, newline-free, one datagram each):
  {"t":"hello","name":...}
  {"t":"start","seed":int,"arena":str,"tick":0}
  {"t":"deploy","tick":int,"key":str,"x":float,"y":float,"team":int}
  {"t":"ping","tick":int}
This module has no pygame dependency so it can be unit-tested headless.
"""

import socket, json, threading, time, random

try:
    import wire
    _WIRE = True
except Exception:
    _WIRE = False
    wire = None

DEFAULT_PORT = 7777
INPUT_DELAY  = 4          # ticks between issuing an input and applying it

# ── shared card index for compact wire encoding ────────────────────────────
# Both peers build the identical key↔id table (sorted), so a deploy travels as a
# 2-byte card index instead of a string. Set once the card set is known; until
# then messages fall back to JSON, so nothing breaks if it's never set.
_KEY2ID = None
_ID2KEY = None

def set_card_index(keys):
    """Install the shared card index (call on both peers with the same keys)."""
    global _KEY2ID, _ID2KEY
    if _WIRE and keys:
        _KEY2ID, _ID2KEY = wire.build_card_index(keys)

def _enc(msg):
    """Encode a message to bytes — compact binary when possible, else JSON."""
    if _WIRE:
        try:
            return wire.encode(msg, _KEY2ID)
        except Exception:
            pass
    return json.dumps(msg).encode()

def _dec(data):
    """Decode bytes to a message dict (handles binary frames + legacy JSON)."""
    if _WIRE:
        try:
            return wire.decode(data, _ID2KEY)
        except Exception:
            pass
    try:
        return json.loads(data.decode())
    except Exception:
        return None


def seat_team(seat):
    """Seats 0,1 → team 0 (bottom); seats 2,3 → team 1 (top)."""
    return 0 if seat < 2 else 1


class NetPeer:
    """Base class holding the shared socket plumbing."""
    def __init__(self):
        self.sock = None
        self.running = False
        self.peer_addr = None
        self.inbox = []                 # list of decoded messages (thread-safe via lock)
        self.lock = threading.Lock()
        self.connected = False
        self.last_rx = 0.0

    def _listen_loop(self):
        while self.running:
            try:
                data, addr = self.sock.recvfrom(2048)
                msg = _dec(data)
                # discovery ping → a host answers with an announce; others ignore
                if msg.get("t") == "discover":
                    if getattr(self, "is_host", False):
                        info = {"t": "announce", "name": getattr(self, "host_name", "Paperbound Table"),
                                "mode": getattr(self, "match_mode", "1v1"),
                                "port": getattr(self, "port", DEFAULT_PORT),
                                "seats_open": len(self._free_seats()) if hasattr(self, "_free_seats") else 0}
                        try: self.sock.sendto(_enc(info), addr)
                        except OSError: pass
                    continue
                msg["_from"] = addr          # tag the sender for the host router
                self.peer_addr = addr
                self.last_rx = time.time()
                if msg.get("t") == "hello":
                    self.connected = True
                if msg.get("t") == "hb":
                    # keep-alive only — refreshes last_rx above; don't queue it
                    self.connected = True
                    continue
                # trust-handshake replies to a client (host asked to approve us)
                if msg.get("t") == "await":
                    self.connected = True
                    if hasattr(self, "awaiting"):
                        self.awaiting = True
                    continue
                if msg.get("t") == "denied":
                    if hasattr(self, "denied"):
                        self.denied = True
                    continue
                if msg.get("t") == "start" and hasattr(self, "awaiting"):
                    self.awaiting = False        # approved → the match is starting
                with self.lock:
                    self.inbox.append(msg)
            except socket.timeout:
                continue
            except (OSError, ValueError):
                continue

    def poll(self):
        """Return and clear all pending messages."""
        with self.lock:
            msgs = self.inbox[:]
            self.inbox.clear()
        return msgs

    def send(self, msg):
        if self.sock and self.peer_addr:
            try:
                self.sock.sendto(_enc(msg), self.peer_addr)
            except OSError:
                pass

    def heartbeat(self, now=None):
        """Send a lightweight keep-alive at most a few times a second so the
        peer can tell a quiet-but-alive link from a dropped one. Call each frame;
        it self-throttles."""
        now = now or time.time()
        if now - getattr(self, "_last_hb_tx", 0.0) >= 0.5:
            self._last_hb_tx = now
            self.send({"t": "hb"})

    def link_dead(self, timeout=6.0, now=None):
        """True if we've heard nothing from the peer for `timeout` seconds after
        a connection was established — i.e. the opponent's app closed / crashed /
        left the network. Used to auto-forfeit on connection close."""
        if not self.connected and self.last_rx == 0.0:
            return False                      # never connected yet; not "dead"
        now = now or time.time()
        return (now - self.last_rx) > timeout

    def stop(self):
        self.running = False
        if self.sock:
            try: self.sock.close()
            except OSError: pass


class Host(NetPeer):
    """Authoritative host for 1v1 or 2v2. Assigns 'seats' to joining devices.

    Seats: 0,1 → team 0 (bottom)   2,3 → team 1 (top)
    The host always owns seat 0. In 2v2 a device may hold two seats (a hotseat
    pair on one team) or single seats can be spread across up to four devices.
    Every deploy is stamped with its seat and rebroadcast to all peers so every
    device runs the identical deterministic lockstep sim.
    """
    def __init__(self, port=DEFAULT_PORT, mode="1v1", host_seats=1, host_name=None,
                 require_approval=True, trusted=None, transport="udp",
                 http_port=None):
        super().__init__()
        self.is_host = True
        self.host_name = host_name or "Paperbound Table"
        self.port = port
        self.transport_kind = transport      # "udp" | "http"
        # ── fleet integration (shared netcode SessionRecord; see fleet.py) ──
        # A paperbound host can announce itself to the shared fleet DHT so friends see it in the
        # games centre and can be handed our dial string. The DHT publisher is injected by a
        # fleet-aware launcher (a running snapcore/OpenDHT node); with none this stays inert and
        # paperbound's own mDNS/UDP discovery is unaffected. Guarded so it never breaks hosting.
        try:
            from fleet import FleetPresence
            self.fleet = FleetPresence(self.host_name)
        except Exception:
            self.fleet = None
        self.seed = random.randint(1, 10_000_000)
        self.arena = "study"
        self.match_mode = mode          # "1v1" | "2v2"
        self.opponent = None            # first opposing client (legacy 1v1)
        self.spectators = set()         # addrs watching only
        # ── trust layer (for public LAN parties) ──
        #   require_approval: the host must OK each joiner before they're seated.
        #   pending: addr -> {"user","role","want","ip"} awaiting host decision.
        #   trusted: set of usernames the host has pre-approved (persisted).
        #   addr_user: addr -> username, for display.
        self.require_approval = require_approval
        self.pending = {}
        self.trusted = set(trusted or [])
        self.addr_user = {}
        self.blocked = set()            # addrs the host explicitly denied
        # Seat→team mapping is always 0,1→team0 and 2,3→team1. In 1v1 only two
        # seats are live but they must sit on OPPOSITE teams, so the pool is
        # [0, 2]; in 2v2 all four seats are live as [0,1,2,3].
        if mode == "2v2":
            self.total_seats = 4
            self.seat_pool = [0, 1, 2, 3]
        else:
            self.total_seats = 2
            self.seat_pool = [0, 2]      # host=team0 seat0, client=team1 seat2
        # host owns seat 0 (single) or seats 0,1 (a team-0 hotseat pair)
        self.host_seats = [0, 1] if (mode == "2v2" and host_seats >= 2) else [0]
        self.seat_owner = {s: "host" for s in self.host_seats}
        self.addr_seats = {}            # addr -> [seats]
        self._http = None               # HTTP transport when transport=="http"
        self._mdns = None
        if transport == "http":
            import transport as _tp
            self.http_port = http_port or _tp.DEFAULT_HTTP_PORT
            self._http = _tp.HTTPHostTransport(port=self.http_port)
            self._http.set_announce(self.host_name, self.match_mode,
                                    len(self._free_seats()))
            # also advertise over mDNS/Avahi so firewalled LANs can still find us
            try:
                self._mdns = _tp.MDNSDiscovery()
                if self._mdns.available:
                    self._mdns.advertise(self.host_name.replace(" ", "-")[:24],
                                         self.http_port,
                                         props={"mode": self.match_mode,
                                                "seats_open": len(self._free_seats()),
                                                "transport": "http"})
            except Exception:
                self._mdns = None
            self.sock = None
            self.running = True
        else:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self.sock.bind(("0.0.0.0", port))
            self.sock.settimeout(0.2)
            self.running = True
            threading.Thread(target=self._listen_loop, daemon=True).start()

    # ── trust helpers ─────────────────────────────────────────────────────
    def note_join_request(self, addr, msg):
        """Record a hello as a pending request the host must approve (unless the
        username is already trusted or approval is disabled). Returns True if the
        joiner should be admitted immediately, False if they're now pending."""
        user = str(msg.get("user") or msg.get("name") or "Player")[:20]
        self.addr_user[addr] = user
        if addr in self.blocked:
            return False
        if not self.require_approval or user in self.trusted:
            return True
        if addr not in self.addr_seats:      # not already admitted
            self.pending[addr] = {"user": user, "role": msg.get("role", "client"),
                                  "want": msg.get("want", 1), "ip": addr[0]}
            # tell the joiner we're waiting on the host
            self.send_to(addr, {"t": "await", "host": self.host_name})
        return False

    def approve(self, addr, remember=True):
        """Host approves a pending joiner; they get seated on the next hello."""
        info = self.pending.pop(addr, None)
        if info and remember:
            self.trusted.add(info["user"])
        return info

    def deny(self, addr):
        info = self.pending.pop(addr, None)
        self.blocked.add(addr)
        self.send_to(addr, {"t": "denied"})
        return info

    def user_of(self, addr):
        return self.addr_user.get(addr, addr[0] if isinstance(addr, tuple) else "Player")

    def _all_peers(self):
        peers = set(self.spectators)
        if self.opponent: peers.add(self.opponent)
        peers.update(self.addr_seats.keys())
        return peers

    def _free_seats(self):
        return [s for s in self.seat_pool if s not in self.seat_owner]

    def all_seats_filled(self):
        return all(s in self.seat_owner for s in self.seat_pool)

    def assign_seats(self, addr, want=1):
        """Give a joining device up to `want` free seats. When want==2 we try to
        hand out a same-team pair (both on team 0 or both on team 1) so a device
        running local hotseat controls one whole side."""
        free = self._free_seats()
        if want >= 2:
            # find a free same-team pair
            for pair in ([0, 1], [2, 3]):
                if pair[0] in free and pair[1] in free:
                    give = pair; break
            else:
                give = free[:1]
        else:
            give = free[:1]
        for s in give:
            self.seat_owner[s] = addr
        if give:
            self.addr_seats.setdefault(addr, []).extend(give)
        return give

    def register(self, addr, role):
        if role == "spectator":
            self.spectators.add(addr)
        else:
            self.opponent = addr        # keep legacy pointer for 1v1 relay code

    def send_to(self, addr, msg):
        if self._http is not None:
            self._http.send_bytes(addr, _enc(msg))
            return
        try:
            self.sock.sendto(_enc(msg), addr)
        except OSError:
            pass

    def _pump_http(self):
        """Pull inbound HTTP messages into the shared inbox (host side). Called
        from poll() so the game's existing poll loop sees them transparently."""
        if self._http is None:
            return
        for cid, data in self._http.recv():
            try:
                m = _dec(data)
            except ValueError:
                continue
            m["_from"] = cid
            self.last_rx = time.time()
            if m.get("t") == "hb":
                continue
            with self.lock:
                self.inbox.append(m)
        # keep the announce fresh (seats change as players join)
        try:
            self._http.set_announce(self.host_name, self.match_mode, len(self._free_seats()))
        except Exception:
            pass

    def broadcast(self, msg, exclude=None):
        for addr in self._all_peers():
            if addr != exclude:
                self.send_to(addr, msg)

    def announce_to_fleet(self, publish=None, ip=None):
        """Announce this host to the shared fleet so friends discover it via the games centre.
        `publish(key, value_bytes)` writes to the fleet DHT (injected by a fleet node); `ip` is our
        advertised LAN address. Best-effort: never raises into the game loop, and a no-op without a
        fleet layer/publisher, so paperbound stays fully playable standalone. Returns the session
        token (which a friend's "join" resolves) or None."""
        if not getattr(self, "fleet", None):
            return None
        try:
            if publish is not None:
                self.fleet._publish = publish
            addr = f"{ip or '0.0.0.0'}:{self.port}"
            return self.fleet.announce_host(self.transport_kind, addr)
        except Exception:
            return None

    def start_match_for(self, addr, role, seats=None):
        """Send the start packet (seed+arena+role+seats+mode+ruleset stamp) to one
        joiner. The `stamp` lets the client confirm it's on the same patch set
        before the lockstep sim begins."""
        self.send_to(addr, {"t": "start", "seed": self.seed, "arena": self.arena,
                            "role": role, "mode": self.match_mode,
                            "seats": seats or [], "tick": 0,
                            "stamp": getattr(self, "ruleset_stamp", "")})

    def start_match(self, arena):
        self.arena = arena
        # legacy single-peer helper retained for the self-test
        self.send({"t": "start", "seed": self.seed, "arena": arena, "tick": 0})

    def poll(self):
        self._pump_http()
        return super().poll()

    def send(self, msg):
        # host's own outgoing (e.g. its deploys) fan out to opponent + spectators
        peers = self._all_peers()
        if peers:
            for addr in peers:
                self.send_to(addr, msg)
        elif self._http is None:
            super().send(msg)

    def heartbeat(self, now=None):
        # over HTTP the long-poll itself is the keep-alive; only UDP needs pings
        if self._http is not None:
            return
        super().heartbeat(now)

    def stop(self):
        self.running = False
        if self._http is not None:
            try: self._http.close()
            except Exception: pass
        if getattr(self, "_mdns", None) is not None:
            try: self._mdns.close()
            except Exception: pass
        if self.sock:
            try: self.sock.close()
            except OSError: pass

    @property
    def lan_ips(self):
        return _local_ips()


class Client(NetPeer):
    def __init__(self, host_ip, port=DEFAULT_PORT, spectator=False, want_seats=1,
                 user="Player", transport="udp", http_port=None):
        super().__init__()
        self.spectator = spectator
        self.want_seats = want_seats     # 1 = single player, 2 = hotseat pair
        self.user = str(user or "Player")[:20]
        self.awaiting = False            # True while the host hasn't approved yet
        self.denied = False              # True if the host refused us
        self.transport_kind = transport
        self._http = None
        role = "spectator" if spectator else "client"
        if transport == "http":
            import transport as _tp
            self._http = _tp.HTTPClientTransport(
                host_ip, port=http_port or _tp.DEFAULT_HTTP_PORT,
                user=self.user, role=role, want=want_seats)
            self.peer_addr = (host_ip, http_port or _tp.DEFAULT_HTTP_PORT)
            self.sock = None
            self.running = True
            # the /join POST already carried our hello; nothing else to send now
        else:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.sock.settimeout(0.2)
            self.peer_addr = (host_ip, port)
            self.running = True
            threading.Thread(target=self._listen_loop, daemon=True).start()
            self.say_hello()

    def _pump_http(self):
        if self._http is None:
            return
        for _cid, data in self._http.recv():
            try:
                m = _dec(data)
            except ValueError:
                continue
            self.last_rx = self._http.last_rx() or time.time()
            t = m.get("t")
            if t == "hb":
                continue
            if t == "await":
                self.awaiting = True; continue
            if t == "denied":
                self.denied = True; continue
            if t == "start":
                self.awaiting = False
            with self.lock:
                self.inbox.append(m)

    def poll(self):
        self._pump_http()
        return super().poll()

    def send(self, msg):
        if self._http is not None:
            self._http.send_bytes(None, _enc(msg))
            return
        super().send(msg)

    def heartbeat(self, now=None):
        if self._http is not None:
            return               # long-poll is the keep-alive
        super().heartbeat(now)

    def link_dead(self, timeout=6.0, now=None):
        if self._http is not None:
            lr = self._http.last_rx()
            if lr == 0.0:
                return False
            return (time.time() - lr) > timeout
        return super().link_dead(timeout, now)

    def stop(self):
        self.running = False
        if self._http is not None:
            try: self._http.close()
            except Exception: pass
        if self.sock:
            try: self.sock.close()
            except OSError: pass

    def say_hello(self):
        role = "spectator" if self.spectator else "client"
        self.send({"t": "hello", "name": self.user, "user": self.user,
                   "role": role, "want": self.want_seats})


def scan_for_hosts(port=DEFAULT_PORT, timeout=0.6):
    """Discover hosts on the LAN, trying every channel a firewall might permit:
      1. mDNS/Avahi (if the optional `zeroconf` package is present) — the service
         discovery protocol Linux trusts by default,
      2. the game's own UDP broadcast ping,
    and returns a merged list of {ip, name, mode, port, seats_open, transport}."""
    found = {}
    # 1) mDNS (works where UDP broadcast is filtered)
    try:
        import transport as _tp
        md = _tp.MDNSDiscovery()
        if md.available:
            for t in md.browse(timeout=max(0.8, timeout)):
                props = t.get("props", {})
                found[t["ip"]] = {"ip": t["ip"], "name": t.get("name", "Table"),
                                  "mode": props.get("mode", "1v1"),
                                  "port": t.get("port", port),
                                  "seats_open": int(props.get("seats_open", 0) or 0),
                                  "transport": props.get("transport", "http")}
        md.close()
    except Exception:
        pass
    # 2) UDP broadcast ping (original path)
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        s.settimeout(timeout)
        ping = json.dumps({"t": "discover"}).encode()
        # broadcast to the general LAN broadcast + each interface's subnet bcast
        targets = {("255.255.255.255", port)}
        for ip in _local_ips():
            parts = ip.split(".")
            if len(parts) == 4:
                targets.add((".".join(parts[:3]) + ".255", port))
        for tgt in targets:
            try: s.sendto(ping, tgt)
            except OSError: pass
        end = time.time() + timeout
        while time.time() < end:
            try:
                data, addr = s.recvfrom(2048)
                msg = _dec(data)
                if msg.get("t") == "announce":
                    found.setdefault(addr[0], {"ip": addr[0], "name": msg.get("name", "Table"),
                                      "mode": msg.get("mode", "1v1"),
                                      "port": msg.get("port", port),
                                      "seats_open": msg.get("seats_open", 0),
                                      "transport": "udp"})
            except socket.timeout:
                break
            except (OSError, ValueError):
                continue
        s.close()
    except OSError:
        pass
    return list(found.values())


def probe_http_host(ip, http_port=None, timeout=0.6):
    """Ask a specific IP over HTTP whether it's hosting — the reliable way to
    'find' a host when both broadcast and mDNS are filtered but you know the
    address (the most locked-down networks). Returns a host dict or None."""
    try:
        import transport as _tp
        d = _tp.http_announce(ip, port=http_port or _tp.DEFAULT_HTTP_PORT, timeout=timeout)
        if d:
            return {"ip": ip, "name": d.get("name", "Table"),
                    "mode": d.get("mode", "1v1"),
                    "port": http_port or _tp.DEFAULT_HTTP_PORT,
                    "seats_open": d.get("seats_open", 0), "transport": "http"}
    except Exception:
        pass
    return None


def _local_ips():
    ips = set()
    try:
        hostname = socket.gethostname()
        for info in socket.getaddrinfo(hostname, None):
            ip = info[4][0]
            if "." in ip and not ip.startswith("127."):
                ips.add(ip)
    except OSError:
        pass
    # also try the classic UDP-connect trick to find the primary interface
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        ips.add(s.getsockname()[0])
        s.close()
    except OSError:
        pass
    return sorted(ips) or ["127.0.0.1"]


# ── tiny self-test when run directly: loopback a host+client handshake ──
if __name__ == "__main__":
    h = Host(port=DEFAULT_PORT, require_approval=False)
    c = Client("127.0.0.1", port=DEFAULT_PORT, user="Tester")
    time.sleep(0.3)
    # host should have received a hello and learned the client addr
    for m in h.poll():
        print("host got:", m)
    h.start_match("study")
    time.sleep(0.3)
    for m in c.poll():
        print("client got:", m)
    # exchange a deploy
    c.send({"t": "deploy", "tick": 10, "key": "crane", "x": 640, "y": 500, "team": 1})
    time.sleep(0.2)
    for m in h.poll():
        print("host got deploy:", m)
    print("LAN IPs:", _local_ips())
    h.stop(); c.stop()

    # ── trust layer: untrusted joiner pends → approve → trusted returns ──
    h2 = Host(port=DEFAULT_PORT + 1, require_approval=True)
    j = Client("127.0.0.1", port=DEFAULT_PORT + 1, user="Guest")
    time.sleep(0.35)
    hellos = [m for m in h2.poll() if m.get("t") == "hello"]
    assert hellos, "no hello received"
    addr = hellos[0]["_from"]
    assert h2.note_join_request(addr, hellos[0]) is False, "untrusted should pend"
    assert addr in h2.pending
    time.sleep(0.25)
    assert j.awaiting, "client should be awaiting approval"
    info = h2.approve(addr, remember=True)
    assert info and "Guest" in h2.trusted
    h2.start_match_for(addr, "client", [2]); time.sleep(0.25)
    assert not j.awaiting, "client should be admitted after approval"
    # heartbeat / link-dead
    h2.heartbeat(); j.heartbeat(); time.sleep(0.1)
    assert not h2.link_dead(timeout=2)
    j.stop(); time.sleep(2.2)
    assert h2.link_dead(timeout=2), "should detect dropped link"
    h2.stop()
    print("✅ netplay self-test complete (handshake + trust + heartbeat)")
