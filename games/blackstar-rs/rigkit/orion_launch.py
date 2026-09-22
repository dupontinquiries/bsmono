#!/usr/bin/env python3
"""orion_launch.py — the transform is driven by momentum, not by the pose.

## The idea

Every version so far had the machine stand up from rest, which means it
generates the whole motion itself: a levitating core, a root that rises
because the author said so, a somersault that had to be removed because
nothing motivated it.

Give it an OUTSIDE FORCE and the same somersault becomes the point. A car at
30 m/s that catches its nose pitches forward whether it wants to or not. The
machine does not have to produce that rotation, it has to SURVIVE it — and
landing on your feet out of a flip you did not choose is a different and much
better read than levering yourself upright.

So the root's position and rotation stop being authored and become integrated:

    launch     an impulse at t=0 -- braking, a ramp, or a hit
    flight     ballistic. gravity, and whatever spin the impulse imparted
    land       feet down, the spin arrested by planting them

The POSES still do all the shaping. What changes is that the root is no
longer a hand-written number per key pose; it is wherever the physics put it,
and the poses are what the body does while it is there.

## Why this is testable rather than a vibe

A launch either lands on its feet or it does not. Integrate, check the body is
upright and moving down slowly at touchdown, and the impulse is either right
or it is not. That is a number, and it is the number the previous approach
could never produce.
"""
import math

G = -980.0          # cm/s^2 in the model's units (orion is ~460 long = 4.6 m)


class Launch:
    """One ballistic arc with spin.

    `pitch_rate` is degrees per second about z: positive tips the nose down,
    which is what happens when a moving car's front end catches."""

    def __init__(self, speed=900.0, pitch_rate=200.0, rise=520.0,
                 start=(-60.0, 16.0, 0.0), start_tilt=-90.0):
        self.speed = speed          # forward, +x
        self.pitch_rate = pitch_rate
        self.rise = rise            # upward impulse from the ramp
        self.start = start
        self.start_tilt = start_tilt

    def at(self, t):
        """State at time t seconds after the impulse."""
        x = self.start[0] + self.speed * t
        y = self.start[1] + self.rise * t + 0.5 * G * t * t
        tilt = self.start_tilt + self.pitch_rate * t
        vy = self.rise + G * t
        return dict(pos=(x, y, self.start[2]), tilt=tilt, vy=vy,
                    t=t, airborne=y > self.start[1])

    def flight_time(self):
        """Back to the height it left at. 2*v/g for a symmetric arc."""
        return -2.0 * self.rise / G

    def total_spin(self):
        return self.pitch_rate * self.flight_time()

    def report(self):
        T = self.flight_time()
        apex = self.at(T * 0.5)
        end = self.at(T)
        return dict(
            flight=T, apex_y=apex['pos'][1], distance=end['pos'][0]-self.start[0],
            spin=self.total_spin(), land_tilt=end['tilt'],
            land_vy=end['vy'],
        )


def solve_impulse(target_spin=90.0, target_flight=None, rise=520.0):
    """What pitch rate produces the rotation we need during the arc?

    The machine has to go from face-down (-90) to upright (0), i.e. +90
    degrees. If the flight gives it exactly that, the transformation is paid
    for by the launch and the body only has to arrange itself."""
    L = Launch(rise=rise)
    T = L.flight_time()
    return target_spin / T, T


if __name__ == '__main__':
    print("what the launch has to buy: -90 (face down) -> 0 (upright) = 90 degrees\n")
    print(f"{'rise':>7}{'flight s':>10}{'apex':>8}{'dist':>8}{'pitch/s':>10}{'land vy':>10}")
    for rise in (300, 420, 520, 640, 800):
        rate, T = solve_impulse(90.0, rise=rise)
        L = Launch(rise=rise, pitch_rate=rate)
        r = L.report()
        print(f"{rise:>7}{r['flight']:>10.2f}{r['apex_y']:>8.0f}{r['distance']:>8.0f}"
              f"{rate:>10.0f}{r['land_vy']:>10.0f}")
    print()
    rate, T = solve_impulse(90.0)
    L = Launch(pitch_rate=rate)
    print(f"picked rise=520: {T:.2f}s of flight, {rate:.0f} deg/s, "
          f"lands tilted {L.at(T)['tilt']:.0f} (0 = upright)")
    print("\nthe somersault is no longer something to remove. it is the thing")
    print("that does the work, and the body's job is to land out of it.")
