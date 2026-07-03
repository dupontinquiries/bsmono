# Cybertron: Arena of Gods

A "Smite meets Fall of Cybertron" transforming-robot arena brawler, built on the
exengine engine (now rolled into this codebase and built with CMake).

## Layout
- engine_src/            the exengine engine, vendored as part of this codebase
  - exengine/            engine core + OUR new game modules:
    - coll_chunk.c/.h     destructible & event-gated collision chunks
    - events.c/.h         pub/sub map-event bus
    - transform.c/.h      robot <-> vehicle transform state machine
    - netcode.c/.h        server-authoritative wire protocol
  - lib/                 bundled PhysFS + MojoAL (no system packages needed)
- tests/                 headless module tests (chunk / transform / netcode)
- CMakeLists.txt         builds libexengine (engine + modules), the game, and tests

## Build
    cmake -B build -S .
    cmake --build build -j

Produces:
- build/libexengine.a    engine + all four Cybertron modules as one static lib
- build/game             the game executable, linked against libexengine
- build/{example_usage,test_transform,test_netcode}   module tests

## Test
    cd build && ctest --output-on-failure

All three module tests pass headless (no GPU/display needed) -- suitable for CI
on Mesa desktop and Asahi aarch64 as well.

## Notes on rendering target
The engine currently uses exengine's OpenGL 3.3 (glad) path. glad vendors its own
GL headers, so only a libGL/GLESv2 runtime is needed to link -- keeping this
buildable on Mesa and Asahi (aarch64) without GL dev packages. The planned Vulkan
migration (for the Mesa/Asahi high-fidelity rasterized target) is the remaining
renderer work; all the game-logic modules above are renderer-independent.
