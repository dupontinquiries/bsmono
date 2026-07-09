"""
PAPERBOUND — transport layer.

The lockstep net code (netplay.py) talks to peers through a small *transport*
interface so the same trust/seat/deploy logic can run over different channels.
Two channels ship, tried as fallbacks, chosen to be things a locked-down Linux
mobile system (postmarketOS / Alpine, nftables) will let through:

  • "udp"   — raw UDP datagrams on the game port. Fast and simple, but blocked by
              default firewalls and by many managed/public networks.
  • "http"  — a tiny HTTP relay the HOST runs on a single TCP port. HTTP over TCP
              is a well-understood, connection-oriented protocol that firewalls
              are comfortable allowing (and easy to name in an nft/iptables rule
              or a systemd socket unit). Clients POST messages and long-poll for
              replies. Pure stdlib (http.server + urllib), no extra packages.

Discovery ("who's hosting on this LAN?") also degrades:

  • mDNS / Zeroconf via Avahi (port 5353) when the optional `zeroconf` package is
    present — this is the service-discovery protocol Linux trusts by default, so
    it works where raw UDP broadcast is filtered.
  • otherwise the UDP broadcast ping in netplay.scan_for_hosts().

Bluetooth (RFCOMM) is detected and scaffolded but not fully implemented here —
see BluetoothProbe; it needs BlueZ/PyBluez which aren't stdlib.

Every transport exposes the same minimal duck-typed API used by netplay:
    t.send_bytes(handle, data:bytes)     # host → a specific peer
    t.recv() -> list[(handle, dict)]     # inbound messages since last call
    t.broadcast_bytes(data, exclude=…)   # host → all peers
    t.close()
Handles are opaque peer identifiers (a UDP (ip,port) tuple, or an HTTP client id
string). netplay never inspects them except for display, for which we provide
handle_ip().
"""
import json, socket, threading, queue, time, http.server, urllib.request, uuid, base64

DEFAULT_HTTP_PORT = 8677        # a nameable TCP port for the HTTP channel


def handle_ip(handle):
    """Best-effort human IP/name for a peer handle (UDP tuple or http id)."""
    if isinstance(handle, tuple) and len(handle) >= 1:
        return handle[0]
    return str(handle)[:12]


# ─────────────────────────────── UDP transport ────────────────────────────
class UDPTransport:
    """Thin wrapper over a UDP socket. Handles are (ip, port) tuples."""
    name = "udp"

    def __init__(self, bind_port=None):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if bind_port is not None:
            self.sock.bind(("0.0.0.0", bind_port))
        self.sock.settimeout(0.2)
        self._peer = None
        self._inbox = queue.Queue()
        self._run = True
        threading.Thread(target=self._loop, daemon=True).start()

    def _loop(self):
        while self._run:
            try:
                data, addr = self.sock.recvfrom(4096)
                self._peer = addr
                self._inbox.put((addr, data))
            except socket.timeout:
                continue
            except OSError:
                break

    def connect(self, ip, port):
        self._peer = (ip, port)

    def send_bytes(self, handle, data):
        try:
            self.sock.sendto(data, handle or self._peer)
        except OSError:
            pass

    def broadcast_bytes(self, data, peers, exclude=None):
        for h in peers:
            if h != exclude:
                self.send_bytes(h, data)

    def recv(self):
        out = []
        while True:
            try:
                addr, data = self._inbox.get_nowait()
                out.append((addr, data))
            except queue.Empty:
                break
        return out

    def close(self):
        self._run = False
        try: self.sock.close()
        except OSError: pass


# ─────────────────────────────── HTTP transport ───────────────────────────
# A single-file HTTP relay. The host runs an HTTP server; each client is an HTTP
# client identified by a random id it sends in every request. Messages are JSON.
#
#   POST /join   {user,role,want}          -> {id}
#   POST /msg    {id, msg}                  -> {ok}
#   GET  /poll?id=…                          -> {msgs:[…]}   (short long-poll)
#   GET  /announce                           -> {name,mode,seats_open}  (discovery)
#
# This is deliberately simple and robust: no persistent sockets to keep alive,
# so it survives NAT/proxy timeouts, and it lives on ONE TCP port that's trivial
# to allow in a firewall config.

class _HTTPState:
    """Shared server state (host side): per-client outbound queues + inbound."""
    def __init__(self):
        self.lock = threading.Lock()
        self.out = {}                 # client_id -> [msg,…] waiting to be polled
        self.inbound = queue.Queue()  # (client_id, msg) received from clients
        self.clients = {}             # client_id -> {"user","role","want","ip","last"}
        self.announce = {"name": "Paperbound Table", "mode": "1v1", "seats_open": 1}

    def enqueue(self, cid, msg):
        with self.lock:
            self.out.setdefault(cid, []).append(msg)

    def drain_out(self, cid):
        with self.lock:
            msgs = self.out.get(cid, [])
            self.out[cid] = []
            if cid in self.clients:
                self.clients[cid]["last"] = time.time()
            return msgs


class _HTTPHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):        # silence default stderr logging
        pass

    def _json(self, obj, code=200):
        body = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_json(self):
        try:
            n = int(self.headers.get("Content-Length", 0))
            return json.loads(self.rfile.read(n).decode()) if n else {}
        except Exception:
            return {}

    @property
    def st(self):
        return self.server.state

    def do_GET(self):
        if self.path.startswith("/announce"):
            self._json(self.st.announce); return
        if self.path.startswith("/poll"):
            from urllib.parse import urlparse, parse_qs
            q = parse_qs(urlparse(self.path).query)
            cid = (q.get("id") or [""])[0]
            # brief long-poll: wait up to ~1s for messages so idle traffic is low
            deadline = time.time() + 1.0
            msgs = self.st.drain_out(cid)
            while not msgs and time.time() < deadline:
                time.sleep(0.05); msgs = self.st.drain_out(cid)
            self._json({"msgs": msgs}); return
        self._json({"error": "not found"}, 404)

    def do_POST(self):
        data = self._read_json()
        ip = self.client_address[0]
        if self.path.startswith("/join"):
            cid = uuid.uuid4().hex[:12]
            with self.st.lock:
                self.st.clients[cid] = {"user": str(data.get("user", "Player"))[:20],
                                        "role": data.get("role", "client"),
                                        "want": data.get("want", 1),
                                        "ip": ip, "last": time.time()}
            # deliver the join as a hello into the host's inbound stream
            self.st.inbound.put((cid, {"t": "hello", "user": self.st.clients[cid]["user"],
                                       "name": self.st.clients[cid]["user"],
                                       "role": self.st.clients[cid]["role"],
                                       "want": self.st.clients[cid]["want"]}))
            self._json({"id": cid}); return
        if self.path.startswith("/msg"):
            cid = data.get("id", "")
            msg = data.get("msg", {})
            if cid:
                self.st.inbound.put((cid, msg))
            self._json({"ok": True}); return
        self._json({"error": "not found"}, 404)


class HTTPHostTransport:
    """Host side of the HTTP relay. Handles are client-id strings."""
    name = "http"

    def __init__(self, port=DEFAULT_HTTP_PORT):
        self.port = port
        self.state = _HTTPState()
        self.httpd = http.server.ThreadingHTTPServer(("0.0.0.0", port), _HTTPHandler)
        self.httpd.state = self.state
        self.httpd.daemon_threads = True
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()

    def set_announce(self, name, mode, seats_open):
        self.state.announce = {"name": name, "mode": mode, "seats_open": seats_open}

    def send_bytes(self, handle, data):
        try:
            try:
                payload = json.loads(data.decode())
            except Exception:
                payload = {"__b64__": base64.b64encode(data).decode("ascii")}
            self.state.enqueue(handle, payload)
        except Exception:
            pass

    def broadcast_bytes(self, data, peers, exclude=None):
        for h in peers:
            if h != exclude:
                self.send_bytes(h, data)

    def recv(self):
        out = []
        while True:
            try:
                cid, msg = self.state.inbound.get_nowait()
                if isinstance(msg, dict) and "__b64__" in msg:
                    out.append((cid, base64.b64decode(msg["__b64__"])))
                else:
                    out.append((cid, json.dumps(msg).encode()))
            except queue.Empty:
                break
        return out

    def close(self):
        try: self.httpd.shutdown()
        except Exception: pass


class HTTPClientTransport:
    """Client side of the HTTP relay. Polls the host on a background thread."""
    name = "http"

    def __init__(self, host_ip, port=DEFAULT_HTTP_PORT, user="Player",
                 role="client", want=1):
        self.base = f"http://{host_ip}:{port}"
        self.user = user
        self.role = role
        self.want = want
        self.id = None
        self._inbox = queue.Queue()
        self._run = True
        self._last_rx = 0.0
        threading.Thread(target=self._start, daemon=True).start()

    def _http(self, path, obj=None, timeout=3.0):
        url = self.base + path
        try:
            if obj is not None:
                req = urllib.request.Request(url, data=json.dumps(obj).encode(),
                                             headers={"Content-Type": "application/json"},
                                             method="POST")
            else:
                req = urllib.request.Request(url, method="GET")
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read().decode())
        except Exception:
            return None

    def _start(self):
        # register, then poll forever
        res = self._http("/join", {"user": self.user, "role": self.role, "want": self.want})
        if res and "id" in res:
            self.id = res["id"]
            self._last_rx = time.time()
        while self._run:
            if not self.id:
                res = self._http("/join", {"user": self.user, "role": self.role, "want": self.want})
                if res and "id" in res:
                    self.id = res["id"]; self._last_rx = time.time()
                else:
                    time.sleep(0.5); continue
            res = self._http(f"/poll?id={self.id}", timeout=3.0)
            if res is not None:
                self._last_rx = time.time()
                for m in res.get("msgs", []):
                    self._inbox.put(m)
            else:
                time.sleep(0.2)

    def send_bytes(self, handle, data):
        if self.id:
            try:
                # payloads may be binary (wire codec frames), so carry them as a
                # base64 blob when they aren't plain JSON — keeps the relay generic.
                try:
                    payload = json.loads(data.decode())
                except Exception:
                    payload = {"__b64__": base64.b64encode(data).decode("ascii")}
                self._http("/msg", {"id": self.id, "msg": payload})
            except Exception:
                pass

    def broadcast_bytes(self, data, peers=None, exclude=None):
        self.send_bytes(None, data)

    def recv(self):
        out = []
        while True:
            try:
                m = self._inbox.get_nowait()
                if isinstance(m, dict) and "__b64__" in m:
                    out.append((self.id, base64.b64decode(m["__b64__"])))
                else:
                    out.append((self.id, json.dumps(m).encode()))
            except queue.Empty:
                break
        return out

    def last_rx(self):
        return self._last_rx

    def close(self):
        self._run = False


# ─────────────────────────── mDNS / Avahi discovery ───────────────────────
class MDNSDiscovery:
    """Advertise / browse Paperbound tables over mDNS (Avahi) when the optional
    `zeroconf` package is installed. mDNS (UDP 5353 multicast) is the standard
    service-discovery protocol on Linux and is usually permitted by default, so
    it works where the game's own UDP broadcast is filtered. No-ops cleanly when
    zeroconf isn't available."""
    SERVICE = "_paperbound._tcp.local."

    def __init__(self):
        self.zc = None
        self._info = None
        try:
            import zeroconf                      # optional dependency
            self._mod = zeroconf
            self.available = True
        except Exception:
            self._mod = None
            self.available = False

    def advertise(self, name, port, props=None):
        if not self.available:
            return False
        try:
            from zeroconf import ServiceInfo
            ip = socket.gethostbyname(socket.gethostname())
            self.zc = self._mod.Zeroconf()
            self._info = ServiceInfo(
                self.SERVICE, f"{name}.{self.SERVICE}",
                addresses=[socket.inet_aton(ip)], port=port,
                properties={k: str(v) for k, v in (props or {}).items()})
            self.zc.register_service(self._info)
            return True
        except Exception:
            return False

    def browse(self, timeout=1.5):
        """Return a list of {ip,port,name,props} tables seen on the LAN."""
        if not self.available:
            return []
        found = []
        try:
            from zeroconf import ServiceBrowser
            zc = self._mod.Zeroconf()

            class _L:
                def add_service(self, zc_, type_, nm):
                    info = zc_.get_service_info(type_, nm, timeout=1000)
                    if info and info.addresses:
                        found.append({
                            "ip": socket.inet_ntoa(info.addresses[0]),
                            "port": info.port,
                            "name": nm.split(".")[0],
                            "props": {k.decode(): v.decode() for k, v in
                                      (info.properties or {}).items()}})
                def update_service(self, *a): pass
                def remove_service(self, *a): pass

            ServiceBrowser(zc, self.SERVICE, _L())
            time.sleep(timeout)
            zc.close()
        except Exception:
            pass
        return found

    def close(self):
        try:
            if self.zc and self._info:
                self.zc.unregister_service(self._info)
            if self.zc:
                self.zc.close()
        except Exception:
            pass


def http_announce(ip, port=DEFAULT_HTTP_PORT, timeout=0.6):
    """Ask a specific host (over HTTP) whether it's running a table. Lets you
    'scan' a known IP even when broadcast/mDNS are blocked."""
    try:
        with urllib.request.urlopen(f"http://{ip}:{port}/announce", timeout=timeout) as r:
            d = json.loads(r.read().decode())
            d.update(ip=ip, port=port)
            return d
    except Exception:
        return None


# ───────────────────────────── Bluetooth probe ────────────────────────────
class BluetoothProbe:
    """Detects whether a Bluetooth RFCOMM channel could be used. Full BT support
    needs BlueZ/PyBluez (not stdlib), so this only reports availability and gives
    the user guidance rather than pretending to pair devices."""
    @staticmethod
    def available():
        # stdlib exposes AF_BLUETOOTH on Linux builds with BlueZ headers
        return hasattr(socket, "AF_BLUETOOTH")

    @staticmethod
    def note():
        if BluetoothProbe.available():
            return ("Bluetooth RFCOMM is present on this system. Pair the two "
                    "phones in your OS settings first, then host over Bluetooth. "
                    "(Requires BlueZ; see the multiplayer guide.)")
        return ("Bluetooth transport needs BlueZ (bluetoothd) and is unavailable "
                "on this build — use the HTTP or UDP channel instead.")


def available_transports():
    """What channels can we offer right now, best-first for firewalled systems."""
    t = ["http", "udp"]               # http first: most firewall-friendly
    return t


if __name__ == "__main__":
    # loopback self-test of the HTTP relay (host <-> client over real TCP)
    host = HTTPHostTransport(port=8688)
    host.set_announce("Test Table", "1v1", 1)
    ann = http_announce("127.0.0.1", 8688)
    assert ann and ann["name"] == "Test Table", ann
    cli = HTTPClientTransport("127.0.0.1", 8688, user="Alice")
    time.sleep(0.6)
    # host should have received the join as a hello
    inbound = host.recv()
    hellos = [json.loads(d.decode()) for _, d in inbound]
    assert any(m.get("t") == "hello" and m.get("user") == "Alice" for m in hellos), hellos
    cid = inbound[0][0]
    # host -> client
    host.send_bytes(cid, json.dumps({"t": "start", "seed": 42}).encode())
    time.sleep(0.6)
    got = [json.loads(d.decode()) for _, d in cli.recv()]
    assert any(m.get("t") == "start" and m.get("seed") == 42 for m in got), got
    # client -> host
    cli.send_bytes(None, json.dumps({"t": "deploy", "key": "crane", "tick": 5}).encode())
    time.sleep(0.4)
    got2 = [json.loads(d.decode()) for _, d in host.recv()]
    assert any(m.get("t") == "deploy" for m in got2), got2
    cli.close(); host.close()
    mdns = MDNSDiscovery()
    bt = BluetoothProbe.available()
    print(f"✅ transport self-test: HTTP relay round-trip OK (announce+join+"
          f"bidirectional msgs). mDNS available: {mdns.available}, "
          f"bluetooth present: {bt}")
