"""flyer_panels.py -- where the welded body divides, read off its profile.

    |z| > 125   the wings: outboard of the nacelles (body thickness drops to a
                flat panel with vertices only at root and tip)
    |z| 50-125  nacelles below y 95, twin tail fins above (fins reach y 142)
    |z| < 50    fuselage: nose ahead of x 110, canopy on top from -110 to 50,
                the rest mid and rear fuselage
"""
PLANES = [(2, 125.0), (2, -125.0), (2, 50.0), (2, -50.0), (2, 0.0),
          (1, 95.0), (0, 110.0), (0, -110.0), (0, 50.0)]


def classify(c):
    x, y, z = c
    side = 'l' if z < 0 else 'r'
    az = abs(z)
    if az > 125.0:
        return 'door_' + side            # WINGS: onto the back, splayed
    if az > 50.0:
        return ('fender_' + side) if y > 95.0 else ('quarter_' + side)   # fins / nacelles
    if x > 110.0:
        # the nose cone stays WHOLE: split, each half was a long narrow sliver
        # that sat on the chest as an angled plank -- a jet's nose is not a
        # car's wide flat bonnet
        return 'nose'
    if -110.0 < x < 50.0 and y > 95.0:
        return 'glass_front'             # canopy
    return 'roof' if x >= -110.0 else 'deck'
