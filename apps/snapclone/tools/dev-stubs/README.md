# dev-stubs — headers for syntax-checking without unbuildable deps

These are NOT real library headers. They declare exactly the API surface
SnapClone uses, with matching signatures, so DHT-gated C++ can be checked with
`g++ -fsyntax-only` in environments where the real dependency can't be built.

## opendht_c.h
The real OpenDHT is built from a git submodule (thirdparty/opendht) that isn't
always fetchable (offline / network-restricted CI). This stub lets you
syntax-check `src/network/JamiAdapter_dht_impl.cpp` and the
`SNAPCLONE_OPENDHT_BACKEND`-gated branches without it.

Usage (syntax check only — will NOT link):
```
QTINC=/usr/include/x86_64-linux-gnu/qt6
g++ -fsyntax-only -std=c++20 -DSNAPCLONE_OPENDHT_BACKEND \
    -I tools/dev-stubs \
    -I$QTINC -I$QTINC/QtCore ... \
    -I include -I src \
    src/network/JamiAdapter_dht_impl.cpp
```
If you extend the project's use of the opendht C API, add the new declarations
here too (keep signatures matching opendht/c/opendht_c.h).
