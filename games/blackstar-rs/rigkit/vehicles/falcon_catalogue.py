"""falcon_catalogue.py -- the 1973 Ford Falcon XB GT hardtop, authored by hand.

Source: imported_car_ford_falcon_gt_1973.iqm from the fleet bundle, the
ORIGINAL rather than the roster's baked fold -- that one had already been
through an automated transform and carried its mistakes (the Ferrari's tyres
had ended up under the middle of the car).

Frame is orion's, so orion's rig, cladding and fold volumes apply unchanged:
x long with the nose at +x, y up with the ground at 0, z lateral with LEFT at
-z. Scaled to orion's 458 length. In the file it lies on its side -- z up, y
lateral -- and (x,y,z)->(x,z,-y) is a proper rotation, so nothing is mirrored.

Nose direction was not assumed. The +x end carries 3x the geometry at bonnet
height (grille, lamps, bumper), and the profile confirms it: long flat bonnet
to +x, fastback sloping to -x.

Ids are from vehicles/isl.py: sorted by vertex count, ties broken on centroid, so
a number here means one island and nothing else.

Vocabulary is ORION's group names. That is the seam: this file says which
islands are which named part; orion_cladding.py says where each named part
goes on the robot, and is reused unchanged.
"""

CATALOGUE = {
    # ---- the body: ONE welded shell, 458 long. Orion's bonnet, roof and
    # doors were separate islands; the Falcon's are all this. It is cut into
    # panels along authored planes in falcon_panels.py rather than kept whole.
    'shell':      [0],

    # ---- wheels. Front axle x=+144, rear x=-123; left is -z.
    'wheel_fl':   [10, 33],     # tyre, rim
    'wheel_fr':   [8, 35],
    'wheel_rl':   [9, 32],
    'wheel_rr':   [7, 34],

    # ---- front end
    'wing':       [13],          # front bumper
    'lamp':       [2, 3, 14, 19, 25, 30],
    'scoop':      [4, 5],        # the GT's bonnet scoops
    'radiator':   [38],          # engine block

    # ---- rear end
    'trim':       [1, 11],       # tail panel, rear bumper
    'exhaust':    [17, 18],      # twin pipes
    'undertray':  [20, 39],      # tank, diff

    # ---- structure
    'floorpan':   [12, 28, 36, 37],
    'frame_l':    [21],          # rear axle ends, inboard of the wheels
    'frame_r':    [24],
    'susp_l':     [],
    'susp_r':     [],

    # ---- the cabin. Right-hand drive: the steering wheel is at z=+41, which
    # the geometry states on its own -- it is an Australian car.
    'cockpit':    [6, 15, 16, 26, 27, 29, 31],   # dash, wheel, console, bench
    'seat_l':     [22],
    'seat_r':     [23],
}

# Everything not named rides with whatever named part it is nearest to, with
# the same distance cap orion uses: a clip too far from anything is not
# attached to it.
CAP = 46.0
