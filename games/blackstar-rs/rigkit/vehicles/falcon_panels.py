"""falcon_panels.py -- where the Falcon's welded shell divides into panels.

Every plane is read off the roofline, not guessed:

    x  +55   windscreen base -- height drops from 126 to 97 here
    x  -50   rear edge of the door (a two-door hardtop's doors are long)
    x -110   base of the rear window, where the fastback starts to fall
    y  105   the roof sits above this
    y   80   top surfaces (bonnet, boot) sit above this; the flanks below
    z    0   the centreline -- the bonnet parts here, so it can crack in two
    z  +-68  where the bonnet and boot lid end and the fenders begin

Names are orion's vocabulary, so orion_cladding.py places them unchanged.
"""

# z +-40 too: the doors are classified at |z| > 40, and classifying without
# cutting there left the doors' inner edge straddling the line -- a sawtooth
# by the old mechanism, just in a place that was harder to see.
PLANES = [(0, 55.0), (0, -50.0), (0, -110.0),
          (1, 105.0), (1, 80.0),
          (2, 0.0), (2, 68.0), (2, -68.0), (2, 40.0), (2, -40.0),
          (2, 74.0), (2, -74.0)]


# The Falcon was modelled WITHOUT a windscreen: a 160-wide opening at the base
# of the roof, and it read as a hole for the whole car phase. The other shell
# openings stay open on purpose -- the grille and tail are covered by their
# own parts, and the rest are wheel wells and floor seams.
def windscreen(centre, size):
    x, y, z = centre
    return 30.0 < x < 80.0 and y > 95.0 and size[2] > 120.0


def classify(c):
    x, y, z = c
    left = z < 0.0
    side = 'l' if left else 'r'
    if y > 105.0 and -110.0 < x < 55.0:
        return 'roof'
    if x > 55.0:
        if y > 80.0 and abs(z) < 68.0:
            return 'bonnet_' + side          # parts down the middle
        return 'fender_' + side
    if x < -110.0:
        if y > 80.0 and abs(z) < 68.0:
            return 'deck'                     # boot lid
        # REAR QUARTER, not "arch". Orion's arch is a small wheel-arch tube
        # that rides an upper arm; mapping the Falcon's whole quarter panel to
        # that name carried the rule onto a panel five times its size, and
        # each one sat 104 long on a shoulder. It is sheet metal: it reclaims.
        return 'quarter_' + side
    if -50.0 < x <= 55.0:
        # THE DOOR IS ITS OUTER SKIN. Taking everything past |z| 40 gave the
        # doors their interior cards too, 81 deep where the skin is a few
        # units -- and every one of those extra triangles stayed on the robot.
        if abs(z) > 74.0:
            return 'door_' + side
        return 'floorpan'
    return 'quarter_' + side if abs(z) > 40.0 else ('roof' if y > 80.0 else 'floorpan')
