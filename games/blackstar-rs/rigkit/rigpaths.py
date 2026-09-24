"""rigpaths -- every path the rig kit reads or writes, relative to the kit.

The pipeline was written in a sandbox and read its models from /tmp and an
upload folder, wrote viewers to an outputs folder, and found the forge by a
relative 'forge' that only worked from one working directory. Everything now
resolves from here: inputs/ (the source models), out/ (built viewers, not
committed), and the repo's own tools/forge.
"""
import os
import sys

KIT = os.path.dirname(os.path.abspath(__file__))
INPUTS = os.path.join(KIT, 'inputs')
OUT = os.path.join(KIT, 'out')
FORGE = os.path.normpath(os.path.join(KIT, '..', '..', '..', 'tools', 'forge'))


def inp(name):
    return os.path.join(INPUTS, name)


def out(name):
    os.makedirs(OUT, exist_ok=True)
    return os.path.join(OUT, name)


for p in (FORGE, KIT):
    if p not in sys.path:
        sys.path.insert(0, p)
