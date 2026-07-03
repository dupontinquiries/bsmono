add_subdirectory(core)
add_subdirectory(debug)
add_subdirectory(network)
add_subdirectory(storage)
add_subdirectory(crypto)
add_subdirectory(face)
add_subdirectory(ui)

# ─── Main Executable ─────────────────────────────────────────────────────────
qt_add_executable(SnapClone
    main.cpp
)

target_link_libraries(SnapClone
    PRIVATE
        snapclone_warnings
        snapclone_core
        snapclone_debug
        snapclone_network
        snapclone_storage
        snapclone_crypto
        snapclone_face
        snapclone_ui
        Qt6::Core
        Qt6::Gui
        Qt6::Widgets
        Qt6::DBus
)

# ─── Rust staticlib duplicate-symbol workaround ────────────────────────────────
#
# We now link multiple independent Rust staticlibs into one C++ binary
# (snapclone-crypto and snapclone-face today; more will follow as we promote
# rlibs to staticlibs for FFI). Each Rust staticlib bundles its own copy of
# `std`, `core`, `alloc`, `rustc_demangle`, etc. The C++ linker then sees
# every Rust standard library symbol defined twice and errors out.
#
# The duplicate symbols ARE byte-identical (same rustc, same target, same
# std version), so picking either copy is safe. `--allow-multiple-definition`
# tells the linker exactly that: "if you see duplicates, take the first one,
# don't fail."
#
# This is the standard pragmatic workaround for multi-staticlib Rust+C++.
# The architecturally-clean fix is to merge all Rust FFI surfaces into a
# single workspace cdylib that exports both crates' extern "C" symbols and
# is the only Rust thing the C++ side links against — but that's a real
# refactor of the Rust workspace, not a build-config tweak. Filed as future
# work; this flag unblocks us in the meantime.
#
# Scope: only the final executable link. Library targets keep strict linking.
#
# Use both the modern LINKER: syntax (CMake 3.13+) and the raw -Wl form.
# Unconditional — SnapClone only ever targets GCC/Clang on Linux/macOS.
# The previous version had an if(CMAKE_CXX_COMPILER_ID MATCHES "GNU|Clang")
# guard around this but it was silently skipping on the phone toolchain
# (compiler ID didn't match the regex), so the flag never reached the
# linker. The guard was unnecessary defensive coding for a portability
# concern we don't actually have. If a future Windows/MSVC port appears,
# /FORCE:MULTIPLE is the equivalent flag — add the conditional then,
# not now.
target_link_options(SnapClone PRIVATE
    "LINKER:--allow-multiple-definition"
    "-Wl,--allow-multiple-definition"
)

set_target_properties(SnapClone PROPERTIES
    WIN32_EXECUTABLE TRUE
    MACOSX_BUNDLE TRUE
)

# ─── Install rules (used by Flatpak and system installs) ─────────────────────
include(GNUInstallDirs)

install(TARGETS SnapClone
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
    BUNDLE  DESTINATION .
)

install(FILES
    ${PROJECT_SOURCE_DIR}/resources/com.snapclone.SnapClone.desktop
    DESTINATION ${CMAKE_INSTALL_DATADIR}/applications
)

install(FILES
    ${PROJECT_SOURCE_DIR}/resources/com.snapclone.SnapClone.metainfo.xml
    DESTINATION ${CMAKE_INSTALL_DATADIR}/metainfo
)
