"""hover_panels.py -- where the welded body divides, from its profile
(file orientation, nose +x after mapping).

    x > 170              the nose: low and narrow (y 25..74, half-width 33..76)
    x < -110, y < 45     the lower rear fins, hanging below the tail (to y 6)
    x < -150             the tail
    y > 100              the top deck (the top runs y 116..126)
    the rest             side panels, left/right and front/rear of x 30
"""
PLANES = [(0, 170.0), (0, -110.0), (0, -150.0), (1, 45.0), (1, 100.0), (0, 30.0), (2, 0.0)]


def classify(c):
    x, y, z = c
    side = 'l' if z < 0 else 'r'
    if x > 170.0:
        return 'nose'
    if x < -110.0 and y < 45.0:
        return 'fin_' + side
    if x < -150.0:
        return 'tail'
    if y > 100.0:
        return 'deck'
    return ('side_f' if x > 30.0 else 'side_r') + side
