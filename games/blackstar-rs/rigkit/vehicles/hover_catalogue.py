"""hover_catalogue.py -- Folly, the hover car (roster hoverer_folly).

The roster chassis carried the old automatic fold's generic robot kit: 29
pieces whose vertex counts recur across unrelated chassis. They are stripped,
leaving the car's own 17. The FILE's orientation is the right way up (a first
reading flipped it; it was wrong). The spoiler (its highest material) sits at
the rear, so the nose is -z, mapped into
orion's frame by (x,y,z) -> (z, y, -x) -- a rotation, determinant +1 (the
first mapping swapped two axes, which is a mirror). 458 x 140 x 197.
"""
CATALOGUE = {
    'shell':   [0],                          # the welded body: cut in hover_panels.py
    'spoiler': [6],                          # the rear wing, on top (y 131)
    'dome':    [1],                          # the hover dome under the rear
    'rod_l':   [2, 4], 'rod_r': [3, 5],      # side cannons
    'pod_l':   [8], 'pod_r': [7],            # hover pods (left is -z)
}
CAP = 46.0
