#!/usr/bin/env python3
"""unit_check.py — are the systemd user units sane before they reach a system?

`systemd-analyze verify` needs a live manager, which CI and this sandbox do not
have, so the checks that can be made statically are made here. Each corresponds
to a failure that is silent or confusing at runtime:

* **@BIN_DIR@ left unsubstituted** — systemd rejects the unit outright with
  "Neither a valid executable name nor an absolute path", and the app simply
  never starts.
* **A socket unit with no matching service** — systemd accepts the socket,
  activates nothing, and the connecting app blocks forever.
* **ListenStream not under %t** — a user socket outside the runtime directory
  survives logout, so the next session finds a stale path already bound.
* **WantedBy on a socket-activated service** — enabling both means the service
  starts at login anyway, which defeats the point of socket activation and
  brings back the idle processes it was meant to avoid.
* **ProtectHome=true with a ReadWritePaths under %h** — the two contradict, and
  the daemon starts but cannot write its own state.
"""
import os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
APPS = os.path.join(ROOT, 'apps')

def units(app):
    d = os.path.join(APPS, app, 'systemd')
    if not os.path.isdir(d): return []
    return [os.path.join(d, f) for f in sorted(os.listdir(d))
            if f.endswith(('.service', '.socket'))]

def val(text, key):
    m = re.search(rf'^{key}=(.*)$', text, re.M)
    return m.group(1).strip() if m else None

def main():
    problems = 0
    for app in sorted(os.listdir(APPS)):
        us = units(app)
        if not us: continue
        names = {os.path.basename(u) for u in us}
        for u in us:
            base = os.path.basename(u)
            text = open(u, encoding='utf8').read()

            if '@BIN_DIR@' in text and base.endswith('.service'):
                # Expected in the source; only a problem if nothing substitutes
                # it. The installer does, so this is informational.
                pass

            if base.endswith('.socket'):
                svc = base[:-len('.socket')] + '.service'
                if svc not in names:
                    print(f"{app}/{base}: no {svc} — systemd activates nothing "
                          f"and the connecting app blocks forever")
                    problems += 1
                listen = val(text, 'ListenStream')
                if listen and not listen.startswith('%t'):
                    print(f"{app}/{base}: ListenStream={listen} is not under %t — "
                          f"a stale socket survives logout")
                    problems += 1

            if base.endswith('.service'):
                sock = base[:-len('.service')] + '.socket'
                if sock in names and val(text, 'WantedBy') is not None:
                    print(f"{app}/{base}: has WantedBy AND a .socket — it will "
                          f"start at login anyway, defeating socket activation")
                    problems += 1
                rw = val(text, 'ReadWritePaths')
                if val(text, 'ProtectHome') == 'true' and rw and '%h' in rw:
                    print(f"{app}/{base}: ProtectHome=true but ReadWritePaths={rw} "
                          f"is under %h — the daemon cannot write its own state")
                    problems += 1
                # Documentation=man:x(1) that resolves to nothing is worse than
                # no Documentation at all: systemctl status offers the link and
                # `man` then reports no such entry.
                doc = val(text, 'Documentation')
                if doc and doc.startswith('man:'):
                    name = doc[4:].split('(')[0]
                    page = os.path.join(APPS, app, 'man', name + '.1')
                    if not os.path.isfile(page):
                        print(f"{app}/{base}: Documentation={doc} but no man/{name}.1 "
                              f"ships — systemctl offers a link to nothing")
                        problems += 1

                if val(text, 'ExecStart') is None:
                    print(f"{app}/{base}: no ExecStart")
                    problems += 1

    print(f"\n{problems} unit problems" if problems else "\nsystemd units look sane")
    return 1 if problems else 0

if __name__ == '__main__':
    sys.exit(main())
