"""
PAPERBOUND — wire codec.

Shrinks the bytes that cross the network and cuts latency. The key insight: both
peers already share identical card data (the ruleset stamp handshake proves it),
so we never need to send a card *definition* or even its string key — we send a
small integer INDEX into a table both sides build the same way. Combined with
binary packing of the hot per-tick messages, a deploy drops from a ~70-byte JSON
blob to ~10 bytes.

Layers:
  • A deterministic card index — `build_card_index(keys)` → {key:idx} + [idx→key],
    identical on both ends because it's sorted. IDs travel, not strings.
  • Binary framing for the frequent messages (deploy / ack / heartbeat / ping):
    one opcode byte + packed fields via struct. No JSON, no field names.
  • JSON-fallback (optionally gzipped) for rare, structured messages
    (start / join / trust / patch index) where flexibility beats a few bytes.

`encode(msg, idx)` / `decode(buf, id2key)` are the only entry points the transport
needs; everything self-describes via the first byte, so old JSON still round-trips.
"""
import json, struct, gzip

# opcodes for binary-framed messages (first byte)
OP_JSON   = 0x00      # rest is UTF-8 JSON (structured / rare messages)
OP_GZJSON = 0x01      # rest is gzip'd UTF-8 JSON (large messages)
OP_DEPLOY = 0x10      # tick:u32, cardidx:u16, x:i16, y:i16, team:u8
OP_ACK    = 0x11      # tick:u32
OP_HB     = 0x12      # (no body) heartbeat
OP_PING   = 0x13      # (no body)
OP_INPUTS = 0x14      # batched deploys on one tick: tick:u32, n:u8, then n×(cardidx:u16,x:i16,y:i16,team:u8)
OP_BUNDLE = 0x15      # nd:u8 deploys (each seq:u32,tick:u32,cardidx:u16,x:i16,y:i16,team:u8) + na:u8 acks (u32 each)

_BDEP = struct.Struct("<IIHhhB")      # seq, tick, cardidx, x, y, team (in a bundle)

_DEPLOY = struct.Struct("<IIHhhB")    # tick, seq, cardidx, x, y, team
_ONE    = struct.Struct("<HhhB")      # cardidx, x, y, team (inside a batch)
_ACK    = struct.Struct("<I")
_GZ_MIN = 220         # only gzip JSON above this size (below it, overhead loses)


# ── the shared card index ──────────────────────────────────────────────────
def build_card_index(keys):
    """Deterministic key↔index tables from a set of card keys. Sorted so both
    peers, given the same ruleset, build the identical mapping."""
    ordered = sorted(keys)
    key2id = {k: i for i, k in enumerate(ordered)}
    return key2id, ordered


# ── encode ─────────────────────────────────────────────────────────────────
def _clamp16(v):
    return int(max(-32000, min(32000, round(v))))


def encode(msg, key2id=None):
    """Serialize a message dict to bytes. Uses a binary frame for the hot
    messages when a card index is available, else JSON (gzipped if large)."""
    t = msg.get("t")
    if key2id is not None:
        if t == "deploy" and msg.get("key") in key2id:
            return bytes([OP_DEPLOY]) + _DEPLOY.pack(
                int(msg.get("tick", 0)) & 0xffffffff, int(msg.get("seq", 0)) & 0xffffffff,
                key2id[msg["key"]],
                _clamp16(msg.get("x", 0)), _clamp16(msg.get("y", 0)),
                int(msg.get("team", 0)) & 0xff)
        if t == "inputs" and msg.get("items"):
            items = msg["items"]; seqs = msg.get("seqs", [])
            keep = [(it, seqs[i] if i < len(seqs) else 0)
                    for i, it in enumerate(items) if it.get("key") in key2id]
            if keep:
                body = bytes([OP_INPUTS]) + struct.pack("<IB",
                    int(msg.get("tick", 0)) & 0xffffffff, min(len(keep), 255))
                for it, sq in keep[:255]:
                    body += struct.pack("<I", int(sq) & 0xffffffff)
                    body += _ONE.pack(key2id[it["key"]], _clamp16(it.get("x", 0)),
                                      _clamp16(it.get("y", 0)), int(it.get("team", 0)) & 0xff)
                return body
        if t == "bundle":
            deps = [d for d in msg.get("deploys", []) if d.get("key") in key2id]
            acks = msg.get("acks", [])[:255]
            body = bytes([OP_BUNDLE]) + struct.pack("<B", min(len(deps), 255))
            for d in deps[:255]:
                body += _BDEP.pack(int(d.get("seq", 0)) & 0xffffffff,
                                   int(d.get("tick", 0)) & 0xffffffff, key2id[d["key"]],
                                   _clamp16(d.get("x", 0)), _clamp16(d.get("y", 0)),
                                   int(d.get("team", 0)) & 0xff)
            body += struct.pack("<B", len(acks))
            for a in acks:
                body += struct.pack("<I", int(a) & 0xffffffff)
            return body
    if t == "ack" and "seqs" in msg:
        seqs = msg["seqs"][:255]
        return bytes([OP_ACK]) + struct.pack("<B", len(seqs)) + b"".join(
            struct.pack("<I", int(s) & 0xffffffff) for s in seqs)
    if t == "ack" and "tick" in msg:
        return bytes([OP_ACK]) + struct.pack("<B", 0) + _ACK.pack(int(msg["tick"]) & 0xffffffff)
    if t == "hb":
        return bytes([OP_HB])
    if t == "discover" or t == "ping":
        return bytes([OP_PING])
    # structured / rare → JSON, gzipped only when it actually helps
    raw = json.dumps(msg, separators=(",", ":")).encode("utf-8")
    if len(raw) >= _GZ_MIN:
        return bytes([OP_GZJSON]) + gzip.compress(raw, 6)
    return bytes([OP_JSON]) + raw


# ── decode ─────────────────────────────────────────────────────────────────
def decode(buf, id2key=None):
    """Bytes → message dict. Inverse of encode(); the first byte selects the
    frame. Unknown/short buffers fall back to a best-effort JSON parse."""
    if not buf:
        return None
    op = buf[0]
    body = buf[1:]
    if op == OP_JSON:
        return json.loads(body.decode("utf-8"))
    if op == OP_GZJSON:
        return json.loads(gzip.decompress(body).decode("utf-8"))
    if op == OP_DEPLOY:
        tick, seq, cid, x, y, team = _DEPLOY.unpack(body)
        key = id2key[cid] if id2key and cid < len(id2key) else None
        return {"t": "deploy", "tick": tick, "seq": seq, "key": key,
                "x": float(x), "y": float(y), "team": team}
    if op == OP_INPUTS:
        tick, n = struct.unpack_from("<IB", body, 0)
        off = struct.calcsize("<IB")
        items = []; seqs = []
        for _ in range(n):
            sq = struct.unpack_from("<I", body, off)[0]; off += 4
            cid, x, y, team = _ONE.unpack_from(body, off); off += _ONE.size
            seqs.append(sq)
            items.append({"key": id2key[cid] if id2key and cid < len(id2key) else None,
                          "x": float(x), "y": float(y), "team": team})
        return {"t": "inputs", "tick": tick, "seqs": seqs, "items": items}
    if op == OP_BUNDLE:
        nd = body[0]; off = 1
        deploys = []
        for _ in range(nd):
            seq, tick, cid, x, y, team = _BDEP.unpack_from(body, off); off += _BDEP.size
            deploys.append({"seq": seq, "tick": tick,
                            "key": id2key[cid] if id2key and cid < len(id2key) else None,
                            "x": float(x), "y": float(y), "team": team})
        na = body[off]; off += 1
        acks = [struct.unpack_from("<I", body, off + i * 4)[0] for i in range(na)]
        return {"t": "bundle", "deploys": deploys, "acks": acks}
    if op == OP_ACK:
        n = body[0] if body else 0
        if n == 0 and len(body) >= 5:
            return {"t": "ack", "tick": _ACK.unpack_from(body, 1)[0]}
        seqs = [struct.unpack_from("<I", body, 1 + i * 4)[0] for i in range(n)]
        return {"t": "ack", "seqs": seqs}
    if op == OP_HB:
        return {"t": "hb"}
    if op == OP_PING:
        return {"t": "discover"}
    # legacy / unframed: try raw JSON (back-compat with old peers)
    try:
        return json.loads(buf.decode("utf-8"))
    except Exception:
        return None


if __name__ == "__main__":
    keys = ["sabertooth", "crane", "baobab", "wildcard", "gorilla"]
    k2i, i2k = build_card_index(keys)
    # both sides build the same index
    k2i2, i2k2 = build_card_index(list(reversed(keys)))
    assert k2i == k2i2 and i2k == i2k2, "index must be order-independent"

    # deploy: compact + round-trips (now carries a seq for reliability)
    dep = {"t": "deploy", "tick": 123, "seq": 9, "key": "crane", "x": 640.4, "y": 512.9, "team": 1}
    enc = encode(dep, k2i)
    dec = decode(enc, i2k)
    assert dec["key"] == "crane" and dec["tick"] == 123 and dec["team"] == 1 and dec["seq"] == 9
    assert abs(dec["x"] - 640) <= 1 and abs(dec["y"] - 513) <= 1
    json_len = len(json.dumps(dep).encode())
    print(f"deploy: {json_len} B JSON → {len(enc)} B binary ({100*len(enc)//json_len}%)")

    # batched inputs on one tick (with seqs)
    batch = {"t": "inputs", "tick": 200, "seqs": [3, 4], "items": [
        {"key": "crane", "x": 300, "y": 400, "team": 0},
        {"key": "gorilla", "x": 500, "y": 420, "team": 0}]}
    b = encode(batch, k2i); db = decode(b, i2k)
    assert len(db["items"]) == 2 and db["items"][1]["key"] == "gorilla" and db["seqs"] == [3, 4]

    # heartbeat / ack are tiny (ack now carries a seq list)
    assert len(encode({"t": "hb"})) == 1
    assert decode(encode({"t": "ack", "seqs": [7, 8]}))["seqs"] == [7, 8]

    # BUNDLE: many deploys across ticks + piggybacked acks in ONE packet
    bundle = {"t": "bundle", "deploys": [
        {"seq": 10, "tick": 100, "key": "crane", "x": 300, "y": 400, "team": 0},
        {"seq": 11, "tick": 104, "key": "gorilla", "x": 500, "y": 420, "team": 0},
        {"seq": 12, "tick": 108, "key": "baobab", "x": 700, "y": 300, "team": 1}],
        "acks": [5, 6, 7]}
    be = encode(bundle, k2i); bd = decode(be, i2k)
    assert len(bd["deploys"]) == 3 and bd["acks"] == [5, 6, 7]
    assert bd["deploys"][1]["key"] == "gorilla" and bd["deploys"][1]["tick"] == 104
    assert bd["deploys"][2]["seq"] == 12
    # a 3-deploy+3-ack bundle vs sending them as 6 separate packets
    sep = sum(len(encode(d, k2i)) for d in
              [{"t": "deploy", **{k: v for k, v in dd.items()}} for dd in bundle["deploys"]]) \
          + len(encode({"t": "ack", "seqs": bundle["acks"]}))
    print(f"bundle: {len(be)}B for 3 deploys+3 acks (vs {sep}B as separate packets)")

    # structured messages survive as JSON, big ones gzip
    start = {"t": "start", "seed": 42, "arena": "study", "seats": [0], "mode": "1v1",
             "stamp": "a" * 16}
    s = encode(start); assert decode(s)["seed"] == 42
    big = {"t": "patches", "index": [{"name": "Cup " + str(i), "hash": "h" * 12} for i in range(30)]}
    gb = encode(big); assert gb[0] == OP_GZJSON and decode(gb)["index"][0]["name"] == "Cup 0"

    # legacy JSON (no frame) still decodes
    assert decode(json.dumps({"t": "hb"}).encode())["t"] == "hb"
    print(f"✅ wire self-test: deploy {len(enc)}B (was {json_len}B), batch, hb=1B, "
          f"ack round-trip, gzip for big JSON ({len(gb)}B), legacy JSON ok")
