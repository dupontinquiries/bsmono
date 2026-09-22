"""flyer_catalogue.py -- the seeker-jet spaceship (flyer candidate #1).

Source: the fleet bundle's original seeker-spaceship model, #4 (fleet bundle
original). Its LONGEST axis is the wingspan (mirror error 0.006) -- it is
wider than it is long -- so length is z, nose +z (engines at -z, cannons at
+z), up y. (x,y,z)->(z,y,-x) is a proper rotation into orion's frame, scaled
by LENGTH to 458: 458 long, 136 tall, 542 across the wings.

#0 is one welded piece -- fuselage, nacelles, tail fins AND wings (flat
panels with vertices only at root and tip) -- cut in flyer_panels.py.
"""
CATALOGUE = {
    'shell':     [0],
    'engine_l':  [2],  'engine_r': [1],          # jet boots
    'gun_l':     [4, 6], 'gun_r': [3, 5],        # forearm cannons, two a side
    'missile_l': [10, 12], 'missile_r': [9, 11], # dissolve
    'trim':      [7, 8, 33, 34],                 # roundels and tabs
    'cockpit':   list(range(13, 33)),            # canopy frame and fittings
}
CAP = 46.0
