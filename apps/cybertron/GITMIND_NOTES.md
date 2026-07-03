# Ingest notes
Vendored SDL2 binary libs (libSDL2.a, SDL2.dll, *.la) were removed on ingest — SDL
should come from the system package manager (see gitmind's cross-distro system-dep
provisioning). CMakeLists may need a `find_package(SDL2)` pass instead of the bundled lib.
