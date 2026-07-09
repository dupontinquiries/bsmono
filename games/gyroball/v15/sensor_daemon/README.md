# tilt_sensor — raw accelerometer daemon for ball_maze.py

Streams `ACCEL x y z` lines at 50 Hz from the IIO accelerometer subsystem.
Gives ball_maze.py smooth analog control instead of coarse discrete tilt events.

## Quickstart (PostmarketOS / Alpine)

```sh
cd sensor_daemon
sh setup.sh          # installs deps, clones libiio, builds everything
./tilt_sensor | python3 ../ball_maze.py YourName
```

## Manual setup

### Option A — libiio submodule (no apk package needed)

```sh
# Install build tools only
apk add gcc make cmake libxml2-dev libudev-zero-dev git

# Add libiio as a submodule (if this dir is inside a git repo)
git submodule add https://github.com/analogdevicesinc/libiio.git sensor_daemon/libiio
git submodule update --init --depth 1 sensor_daemon/libiio

cd sensor_daemon
make submodule
```

### Option B — system libiio (if packaged)

```sh
# Debian/Ubuntu
apt install libiio-dev gcc make
make

# Alpine (if libiio-dev appears in future)
apk add libiio-dev gcc make
make
```

### Option C — zero-dependency sysfs fallback

No libiio at all — reads directly from `/sys/bus/iio/devices/`.

```sh
make sysfs
./tilt_sensor_sysfs | python3 ../ball_maze.py YourName
```

## Output format

```
TILT_SENSOR_V1          ← sent once on startup so ball_maze.py knows to use analog mode
ACCEL 0.1234 9.7123 0.5678
ACCEL 0.1190 9.6980 0.5701
...
```

Values are in m/s². ball_maze.py normalises by dividing by 9.8.

## Axis mapping (phone held portrait, screen toward you)

| Axis | Direction | Effect |
|------|-----------|--------|
| X    | + = right edge down | ball rolls right |
| Y    | + = top away from you | ball rolls down |
| Z    | + = screen faces up | (unused) |

Use **Flip Horizontal / Flip Vertical** in the in-game Config menu if axes
are inverted for your device's mounting orientation.

## Troubleshooting

**"no accelerometer found"**
```sh
# List IIO devices on your device
ls /sys/bus/iio/devices/
cat /sys/bus/iio/devices/iio:device0/name
```
Add the device name to `ACCEL_NAMES[]` in `tilt_sensor.c` if it's missing.

**cmake can't find libxml2**
```sh
apk add libxml2-dev
```

**udev errors on Alpine**
```sh
apk add libudev-zero-dev
# or build libiio with -DWITH_NETWORK_BACKEND=OFF if udev isn't needed
```
