# Long-term vision

This is for ideas that aren't on the near-term TODO but are worth
keeping in mind as architectural shaping forces. None of this is
committed to. If a near-term decision would close off one of these
directions, we should at least notice.

## The "local data hub" pattern

The architecture decision in ADR-001 (local-only) and ADR-006 / 007
(IPC channels) collectively make `ambient-learner` a **local data
hub** that other apps build on. A few directions this opens up:

### 1. A constellation of small specialized apps

Today, a "weather app" is a monolith: it fetches forecasts, stores
them, shows a UI, and integrates with widgets. The local-data-hub
pattern lets each piece be a separate app on the user's device:

- A *weather sync* app talks to OpenWeather / NOAA / etc. and drops
  observations into our XDG inbox.
- A *forecast UI* renders our map pins as a weather overlay.
- A *home automation* app subscribes to our pressure-trend signal
  and dims the lights when a storm is coming.

We are the data layer. Each piece can be replaced without breaking
the others. The user composes their own stack.

### 2. The "snap" sharing primitive

The snapclone-bus federation hints at a richer pattern: every
device that runs both apps gets a shared whiteboard of map-pin-like
events. Public snaps, shared photos, transit alerts from a
neighbor, etc. There's an interesting privacy story here (federated,
no centralized server) and a UX story (the map becomes a shared
view of the local social world).

We're a small piece of this: a thermal-data publisher and a
geo-located-pin renderer. But the whole concept depends on the
local-IPC pattern being clean, and that's where we put a lot of
effort.

## Sensor reuse for non-temperature purposes

### Touch-input correction

The screen-temp bitfield was originally for "show the user where
their device is hot". The capacitive-touch correction use case
(rain detection, glove finger detection) is **more valuable** — it
makes existing devices more usable in conditions where capacitive
touch normally fails. This is the kind of feature that could change
which mobile-Linux phone someone buys.

A natural extension: publish a touch-coordinate *correction map*
alongside the temperature map. A consumer would subscribe to both:
"here's where the screen is cold (water), here's the suggested
correction kernel (move detected touches by N pixels toward the
nearest warm region)."

### Grip / orientation as a side-channel

The thermal bitfield warms slowly where the hand grips the phone.
Combined with the accelerometer (already polled via iio-sensor-
proxy), this is a passive **handedness detector**. Left-handed users
typically grip with the right side of the phone, right-handed with
the left. Knowing this without ever asking would let UI scrolling
zones, FAB positions, and back-gesture sensitivity adapt
automatically.

### Pocket / dock detection

Uniformly warm screen + low ambient light + accelerometer steady =
in a pocket. Cold + bright + steady = on a desk. Cold + bright +
shaking = walking with it in your hand. These contexts are useful
for things like:

- Suppressing notification sounds when in-pocket.
- Allowing higher-power workloads when docked.
- Disabling auto-lock when actively in-hand.

A small classifier over our existing data could expose
"PhoneContext = {Hand, Desk, Pocket, Docked, Driving}" as a D-Bus
property and a wide variety of apps could subscribe.

## The ML model could do more

Today the neural field is a small NN that maps sensor readings to
ambient temperature. Two directions worth keeping in mind:

### 1. Predictive thermal management

The model already knows the device's thermal state. With a workload
signal (CPU usage, GPU usage, battery current) it could predict
where the device will be in 30 seconds and pre-emptively throttle
or boost. Cooperate with the kernel's thermal governor instead of
fighting it.

### 2. Cross-device transfer learning

If multiple PinePhones publish their (sensor readings, ambient
truth) pairs to a federated learning system, each device benefits
from the others' calibration without sharing raw data. Differential
privacy applies; this is a real research direction.

We could *participate* in this without *running* it: our public
D-Bus already exposes the calibration history. A separate
federated-learning client app could subscribe and contribute on the
user's behalf.

## Mobile Linux as a unified platform

Today there's significant fragmentation between Mobian, postmarketOS,
Plasma Mobile, Phosh, Sxmo, etc. Each has its own conventions,
default app suite, theming. We work on all of them because we don't
care about any specific one's choices.

Long-term, the value of *being a portable mobile-Linux app* compounds
as users move between distros. Stay portable. Avoid taking hard
dependencies on distro-specific things (libhandy is fine; Plasma-
specific is not).

## What we won't grow into

For clarity, these are anti-goals:

- **A cloud service.** Never. We're local-only forever.
- **An Android port.** Different sensor APIs, different IPC,
  different threat model. Use a different app there.
- **A drop-in replacement for `lm_sensors`.** They're sensor
  enumeration tooling; we're a learning predictor. Different scope.
- **A web dashboard.** If a user wants a web view, they run a
  web-app sibling that consumes our IPC.

## Five-year aspirational state

If everything goes well:

1. PinePhone / Librem 5 / similar mobile Linux devices ship with a
   `local-data-hub` namespace that includes us and a few peer apps.
2. Touch input on those devices works in rain through the screen-
   temp correction layer.
3. The model has been retrained on tens of thousands of devices'
   calibration sets and the cold-start prediction error is under
   1°C without user calibration.
4. The snapclone-bus federation has a critical mass of apps so
   "publish to my map" is a verb users say without thinking.

This is aspirational. The Tier 1 / 2 TODOs are real. But these are
the shapes the codebase should leave room to grow into.
