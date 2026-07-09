#!/bin/sh
# Rolling Ball Maze — tilt to steer.
# The tilt read + control model live in the shared sensor-kit crate. Build its driver once from the
# repo root:   cargo build -p sensor-kit --bin tilt-source
# then run this. If the binary isn't on PATH, point at it:  TILT_SOURCE=/path/to/tilt-source ./run.sh
set -eu
DIR="$(cd "$(dirname "$0")" && pwd)"
TILT="${TILT_SOURCE:-tilt-source}"
monitor-sensor --accel 2>&1 | "$TILT" | python3 "$DIR/ball_maze.py" "$@"
