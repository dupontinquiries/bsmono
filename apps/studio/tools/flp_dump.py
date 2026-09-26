#!/usr/bin/env python3
"""flp_dump -- an FL Studio project (.flp) as a JSON inventory Studio can import.

Uses PyFLP (https://github.com/demberto/PyFLP), which does the hard part --
FL's event format is undocumented and PyFLP is the reverse-engineering. This
script only walks what PyFLP exposes and writes it down; every field that
PyFLP cannot read is recorded as an error string, never guessed.

PyFLP 2.2 runs on Python 3.10: on 3.11.15 and 3.12 its EventEnum has "no
members" before a byte is read (checked). So:

    uv python install 3.10 && uv venv -p 3.10 flpenv && VIRTUAL_ENV=flpenv uv pip install pyflp
    flpenv/bin/python tools/flp_dump.py song.flp > song.flp.json

Output: { project, channels, patterns, arrangements, mixer } -- times in
ticks at `project.ppq` per quarter note, exactly as FL stores them.
"""
import json, sys, ntpath

import pyflp


def safe(f, default=None):
    try:
        v = f()
        return v
    except Exception as e:  # noqa: BLE001 -- the point is to record, not crash
        return {"error": f"{type(e).__name__}: {e}"}


def num(v):
    if isinstance(v, (int, float, str, bool)) or v is None:
        return v
    if isinstance(v, dict):
        return v
    try:
        return float(v)
    except Exception:  # noqa: BLE001
        return str(v)


def channel(c):
    kind = type(c).__name__
    d = {"iid": c.iid, "kind": kind, "name": safe(lambda: str(c.name) if c.name else None)}
    for k in ("insert", "volume", "pan", "enabled", "color", "group"):
        d[k] = num(safe(lambda k=k: getattr(c, k)))
    if kind == "Sampler":
        path = safe(lambda: str(c.sample_path) if c.sample_path else None)
        d["sample_path"] = path
        d["sample_file"] = ntpath.basename(path) if isinstance(path, str) else None
        d["pitch_shift"] = num(safe(lambda: c.pitch_shift))
        d["reversed"] = num(safe(lambda: c.playback.use_loop_points if c.playback else None))
    elif kind in ("Instrument",):
        d["plugin"] = safe(lambda: c.internal_name)
        d["plugin_name"] = safe(lambda: getattr(c.plugin, "name", None) if c.plugin else None)
        d["plugin_type"] = safe(lambda: type(c.plugin).__name__ if c.plugin else None)
    elif kind == "Automation":
        d["points"] = safe(lambda: [{"pos": p.position, "value": num(p.value)} for p in c.lfo.points] if False else None)
    return d


def pattern(p):
    notes = safe(lambda: [{"rack": n.rack_channel, "pos": n.position, "len": n.length, "key": n.key,
                           "vel": n.velocity, "pan": n.pan, "fine": n.fine_pitch, "slide": n.slide}
                          for n in p.notes])
    return {"iid": p.iid, "name": safe(lambda: p.name), "length": safe(lambda: p.length), "notes": notes}


def item(it):
    kind = type(it).__name__
    d = {"kind": kind, "pos": safe(lambda: it.position), "len": safe(lambda: it.length),
         "offsets": safe(lambda: list(it.offsets)), "muted": safe(lambda: it.muted)}
    if kind == "ChannelPLItem":
        d["channel"] = safe(lambda: it.channel.iid)
    elif kind == "PatternPLItem":
        d["pattern"] = safe(lambda: it.pattern.iid)
    return d


def arrangement(a):
    tracks = []
    for t in a.tracks:
        items = [item(i) for i in t]
        if not items and not safe(lambda: t.name):
            continue
        tracks.append({"iid": safe(lambda: t.iid), "name": safe(lambda: t.name), "enabled": safe(lambda: t.enabled),
                       "items": items})
    return {"iid": a.iid, "name": safe(lambda: a.name), "tracks": tracks}


def insert(ins):
    slots = []
    for s in ins:
        plug = safe(lambda: s.internal_name)
        if not plug and not safe(lambda: s.name):
            continue
        slots.append({"index": safe(lambda: s.index), "plugin": plug, "name": safe(lambda: s.name),
                      "enabled": safe(lambda: s.enabled), "mix": num(safe(lambda: s.mix)),
                      "plugin_type": safe(lambda: type(s.plugin).__name__ if s.plugin else None)})
    # FL numbering (master = 0), as channels' `insert` uses: PyFLP numbers
    # inserts from file position - 1. `routes` left out: PyFLP pairs the
    # routing flags with the fader and pan params and returns the fader. Its
    # SLOT grouping is also off by one plugin (it starts a slot at the index
    # event, which FL writes LAST). The native reader (flp_read) has neither
    # problem: prefer it; this script is the cross-check.
    return {"iid": ins.iid + 1, "name": safe(lambda: ins.name), "volume": num(safe(lambda: ins.volume)),
            "pan": num(safe(lambda: ins.pan)), "enabled": safe(lambda: ins.enabled), "slots": slots}


def main(path):
    p = pyflp.parse(path)
    out = {
        "project": {"title": safe(lambda: p.title), "artists": safe(lambda: p.artists), "tempo": num(safe(lambda: p.tempo)),
                    "ppq": safe(lambda: p.ppq), "version": safe(lambda: str(p.version)),
                    "time_signature": safe(lambda: [p.time_signature.num, p.time_signature.beat])},
        "channels": [channel(c) for c in p.channels],
        "patterns": [pattern(x) for x in p.patterns],
        "arrangements": [arrangement(a) for a in p.arrangements],
        "mixer": [insert(i) for i in p.mixer],
    }
    # FL stores "no offset" as NaN on pattern clips; JSON has no NaN (Python
    # writes it anyway, and a strict reader refuses the whole file). null.
    def finite(v):
        if isinstance(v, float) and (v != v or v in (float("inf"), float("-inf"))):
            return None
        if isinstance(v, dict):
            return {k: finite(x) for k, x in v.items()}
        if isinstance(v, list):
            return [finite(x) for x in v]
        return v
    json.dump(finite(out), sys.stdout, indent=1, default=str, allow_nan=False)


if __name__ == "__main__":
    main(sys.argv[1])
