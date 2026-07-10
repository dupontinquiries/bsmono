"""
webhost-kit — socket adapter for non-Python games.

The Rust/C games can't be imported into Python, so they front the shared browser/tournament host via
this thin adapter. The game runs a tiny endpoint (a local TCP socket) that speaks newline-delimited
JSON; this adapter forwards the kit's `snapshot`/`apply_action` calls to it. `step()` is a no-op — the
native game runs its own authoritative loop and this side only reflects/pokes it.

Protocol (one JSON object per line, request → response):
    → {"cmd":"snapshot"}                     ← {"ok":true, ...game state...}
    → {"cmd":"action","action":{…},"seat":0} ← {"ok":true}

If the game isn't reachable, snapshot returns {"ok":false,"err":"offline"} and the web UI just shows
"waiting for <game>" — nothing crashes. See games/soldier/src/netview.rs for the reference server.
"""
from __future__ import annotations

import json
import socket
from typing import Optional

from webserve import _AdapterDefaults


class SocketAdapter(_AdapterDefaults):
    def __init__(self, title: str, host: str = "127.0.0.1", port: int = 7788, timeout: float = 0.5):
        self.title = title
        self._addr = (host, port)
        self._timeout = timeout
        self._sock: Optional[socket.socket] = None
        self._buf = b""

    # ── connection (lazy, self-healing) ──
    def _conn(self) -> Optional[socket.socket]:
        if self._sock is not None:
            return self._sock
        try:
            s = socket.create_connection(self._addr, timeout=self._timeout)
            s.settimeout(self._timeout)
            self._sock, self._buf = s, b""
            return s
        except OSError:
            return None

    def _drop(self):
        if self._sock is not None:
            try:
                self._sock.close()
            except OSError:
                pass
        self._sock, self._buf = None, b""

    def _rpc(self, req: dict) -> dict:
        s = self._conn()
        if s is None:
            return {"ok": False, "err": "offline"}
        try:
            s.sendall((json.dumps(req) + "\n").encode())
            # read one line
            while b"\n" not in self._buf:
                chunk = s.recv(4096)
                if not chunk:
                    raise ConnectionError("closed")
                self._buf += chunk
            line, self._buf = self._buf.split(b"\n", 1)
            return json.loads(line)
        except (OSError, ConnectionError, ValueError):
            self._drop()
            return {"ok": False, "err": "offline"}

    # ── GameAdapter seam ──
    def snapshot(self) -> dict:
        snap = self._rpc({"cmd": "snapshot"})
        snap.setdefault("title", self.title)
        return snap

    def apply_action(self, action: dict, seat) -> dict:
        return self._rpc({"cmd": "action", "action": action, "seat": seat})

    def step(self) -> None:
        # native game owns its loop; nothing to do here.
        pass


if __name__ == "__main__":
    # Verify the adapter against an in-process mock server speaking the soldier protocol.
    import threading
    import time

    def mock_server(port):
        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", port))
        srv.listen(1)
        conn, _ = srv.accept()
        buf = b""
        state = {"ok": True, "title": "soldier", "score": 100, "player": {"x": 1.0, "y": 2.0, "z": 3.0, "hp": 87.5}, "enemies": [{"x": 5.0, "y": 0.0, "z": 6.0, "hp": 40.0}]}
        while True:
            while b"\n" not in buf:
                c = conn.recv(4096)
                if not c:
                    return
                buf += c
            line, buf = buf.split(b"\n", 1)
            req = json.loads(line)
            if req.get("cmd") == "snapshot":
                conn.sendall((json.dumps(state) + "\n").encode())
            elif req.get("cmd") == "action":
                state["score"] += 100
                conn.sendall(b'{"ok": true}\n')

    port = 7799
    threading.Thread(target=mock_server, args=(port,), daemon=True).start()
    time.sleep(0.2)
    a = SocketAdapter("soldier", port=port)
    snap = a.snapshot()
    assert snap["ok"] and snap["player"]["hp"] == 87.5 and snap["enemies"][0]["hp"] == 40.0, snap
    r = a.apply_action({"kind": "ping"}, 0)
    assert r["ok"], r
    assert a.snapshot()["score"] == 200, "action was forwarded and reflected"
    # offline behaviour: a dead adapter degrades, never raises
    dead = SocketAdapter("nobody", port=1)  # nothing listening
    assert dead.snapshot() == {"ok": False, "err": "offline", "title": "nobody"}
    print("✅ SocketAdapter self-test: snapshot, action forward, and offline-degrade OK")
