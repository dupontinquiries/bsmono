cmake_minimum_required(VERSION 3.20)
project(SnapClone VERSION 1.0.0 LANGUAGES CXX)

# ─── C++ Standard ────────────────────────────────────────────────────────────
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# ─── Build Options ────────────────────────────────────────────────────────────
# All options must be declared before add_subdirectory() so that
# add_compile_definitions() is visible to all child targets.
option(SNAPCLONE_BUILD_TESTS       "Build unit tests"                        ON)
option(SNAPCLONE_BUILD_TOOLS       "Build standalone dev tools (ringtone_workshop)" OFF)
option(SNAPCLONE_ENABLE_SANITIZERS "Enable ASAN/UBSAN in Debug builds"       OFF)
option(SNAPCLONE_JAMI_BACKEND      "Enable Jami/dhtnet P2P backend"          OFF)
# SNAPCLONE_OPENDHT_BACKEND is declared in the backend section below
option(SNAPCLONE_RUST_CRYPTO       "Use Rust AES-256-GCM crypto backend"     OFF)

# ─── Qt Setup ────────────────────────────────────────────────────────────────
find_package(Qt6 REQUIRED COMPONENTS
    Positioning
    Core
    Gui
    Widgets
    Network
    WebSockets
    Multimedia
    MultimediaWidgets
    Sql
    Concurrent
    DBus
    Svg
)
qt_standard_project_setup()

# ─── Compiler Warnings Interface Library ─────────────────────────────────────
# Using an INTERFACE library keeps warning flags DRY — link against
# snapclone_warnings instead of repeating flags in every target.
add_library(snapclone_warnings INTERFACE)
target_compile_options(snapclone_warnings INTERFACE
    $<$<CXX_COMPILER_ID:GNU,Clang>:
        -Wall -Wextra -Wpedantic
        -Wshadow               # catches accidental variable shadowing
        -Wconversion           # catches implicit narrowing
        -Wnull-dereference     # catches obvious null pointer bugs
        -Wold-style-cast       # enforce C++ casts
    >
    $<$<CXX_COMPILER_ID:MSVC>:/W4 /permissive->
)

if(SNAPCLONE_ENABLE_SANITIZERS AND CMAKE_BUILD_TYPE STREQUAL "Debug")
    message(STATUS "Sanitizers: ON (ASAN + UBSAN)")
    target_compile_options(snapclone_warnings INTERFACE -fsanitize=address,undefined)
    target_link_options(snapclone_warnings INTERFACE    -fsanitize=address,undefined)
endif()

# ─── Peer Discovery Backend ───────────────────────────────────────────────────
#
# Two options for internet-scale peer discovery beyond LAN gossip:
#
#  SNAPCLONE_JAMI_BACKEND=ON
#    Use the system libjami package (full Jami daemon).
#    Best for desktop Linux where jami-daemon-devel is packaged.
#      Ubuntu:  sudo add-apt-repository ppa:savoirfairelinux/jami-nightly
#               sudo apt install libjami-dev
#      Fedora:  sudo dnf install jami-daemon-devel
#
#  SNAPCLONE_OPENDHT_BACKEND=ON
#    Build OpenDHT from source via CMake FetchContent.
#    Best for postmarketOS / Alpine aarch64 where libjami-dev is not packaged.
#    OpenDHT is the DHT layer inside Jami — lighter build, same peer discovery.
#    System dependencies (all in Alpine/postmarketOS repos):
#      apk add gnutls-dev nettle-dev argon2-dev msgpack-cxx-dev
#
#    cmake -B build -DSNAPCLONE_OPENDHT_BACKEND=ON
#
# Both OFF → LAN gossip only (default, zero extra dependencies).

option(SNAPCLONE_OPENDHT_BACKEND "Build OpenDHT from source for DHT peer discovery" OFF)

if(SNAPCLONE_JAMI_BACKEND)
    find_package(PkgConfig REQUIRED)
    # IMPORTED_TARGET creates a proper PkgConfig::jami CMake target that
    # carries include dirs, compile options, and link libraries automatically.
    # Without it, JAMI_INCLUDE_DIRS may be empty even when pkg-config finds
    # the library, because some .pc files put headers in non-standard variables.
    pkg_check_modules(JAMI REQUIRED IMPORTED_TARGET jami)
    add_compile_definitions(SNAPCLONE_JAMI_BACKEND)
    message(STATUS "Jami backend    : ON  (libjami ${JAMI_VERSION})")

elseif(SNAPCLONE_OPENDHT_BACKEND)
    # ── OpenDHT via git submodule ─────────────────────────────────────────────
    # OpenDHT is the DHT layer used by Jami for peer discovery.  We use it
    # directly (without dhtnet or libjami) because:
    #   • Lighter build — just GnuTLS + nettle + msgpack, no pjproject
    #   • aarch64 packages exist on Alpine/postmarketOS
    #   • We only need discovery (userId → host:port); WebSocket handles data
    #   • DhtRunner publishes/looks up values; our existing WS transport connects
    #
    # One-time setup (from repo root):
    #   git submodule add https://github.com/savoirfairelinux/opendht.git third_party/opendht
    #   (or thirdparty/opendht — both are supported)
    #   git submodule update --init --recursive
    #
    # System deps (Alpine/postmarketOS):
    #   apk add gnutls-dev nettle-dev argon2-dev msgpack-cxx-dev

    # OpenDHT source dir. Prefer the monorepo's shared vendor/ copy; fall back
    # to the legacy in-tree submodule names if someone still has them.
    if(EXISTS "${CMAKE_SOURCE_DIR}/../../vendor/opendht/CMakeLists.txt")
        set(OPENDHT_SUBMODULE_DIR "${CMAKE_SOURCE_DIR}/../../vendor/opendht")
    elseif(EXISTS "${CMAKE_SOURCE_DIR}/thirdparty/opendht/CMakeLists.txt")
        set(OPENDHT_SUBMODULE_DIR "${CMAKE_SOURCE_DIR}/thirdparty/opendht")
    else()
        set(OPENDHT_SUBMODULE_DIR "${CMAKE_SOURCE_DIR}/third_party/opendht")
    endif()

    if(NOT EXISTS "${OPENDHT_SUBMODULE_DIR}/CMakeLists.txt")
        message(FATAL_ERROR
            "opendht submodule not found at ${OPENDHT_SUBMODULE_DIR}. "
            "Run: git submodule update --init --recursive")
    endif()

    # ── Disable everything except the core library ────────────────────────────
    set(OPENDHT_TOOLS          OFF CACHE BOOL "" FORCE)
    set(OPENDHT_PYTHON         OFF CACHE BOOL "" FORCE)
    set(OPENDHT_C              OFF CACHE BOOL "" FORCE)
    set(OPENDHT_PEER_DISCOVERY OFF CACHE BOOL "" FORCE)
    set(OPENDHT_HTTP           OFF CACHE BOOL "" FORCE)
    set(OPENDHT_TESTS          OFF CACHE BOOL "" FORCE)

    # BUILD_TESTING is the CTest standard variable.  opendht checks it directly
    # to decide whether to require cppunit — even when OPENDHT_TESTS=OFF.
    # Force it OFF before add_subdirectory so opendht's find for cppunit is skipped.
    # We restore our own tests via SNAPCLONE_BUILD_TESTS further below.
    set(BUILD_TESTING OFF CACHE BOOL "" FORCE)

    # ── msgpack compatibility shim ────────────────────────────────────────────
    # opendht looks for a CMake package called "msgpack-cxx".
    # Fedora/RHEL install msgpack-devel which puts headers in /usr/include but
    # registers the CMake package as "msgpack" (not "msgpack-cxx"), so
    # find_package(msgpack-cxx) fails even when the headers are present.
    # We create a minimal INTERFACE target so opendht's find_package succeeds.
    if(NOT TARGET msgpack-cxx)
        find_path(MSGPACK_INCLUDE_DIR msgpack.hpp
            HINTS /usr/include /usr/local/include)
        if(MSGPACK_INCLUDE_DIR)
            add_library(msgpack-cxx INTERFACE IMPORTED GLOBAL)
            target_include_directories(msgpack-cxx INTERFACE "${MSGPACK_INCLUDE_DIR}")
            # Also set the variables opendht checks after finding msgpack.
            set(msgpack_FOUND    TRUE  CACHE BOOL "" FORCE)
            set(MSGPACK_TARGET   "msgpack-cxx")
            message(STATUS "msgpack headers: ${MSGPACK_INCLUDE_DIR} (INTERFACE target created)")
        else()
            message(WARNING "msgpack.hpp not found — opendht may download it automatically")
        endif()
    endif()

    add_subdirectory("${OPENDHT_SUBMODULE_DIR}" opendht_build EXCLUDE_FROM_ALL)

    # opendht_c.h (the C API wrapper) lives in ${OPENDHT_SUBMODULE_DIR}/c/ and
    # #includes <opendht/def.h> from ${OPENDHT_SUBMODULE_DIR}/include/.
    # We cannot modify INTERFACE_INCLUDE_DIRECTORIES on the opendht target from
    # outside its own CMakeLists (CMake policy CMP0076 forbids it for source paths).
    # Instead, create a thin INTERFACE library that bundles both directories and
    # links opendht transitively. Consumers link opendht_with_c instead of opendht.
    add_library(opendht_with_c INTERFACE)
    target_include_directories(opendht_with_c SYSTEM INTERFACE
        "${OPENDHT_SUBMODULE_DIR}/c"
        "${OPENDHT_SUBMODULE_DIR}/include")
    target_link_libraries(opendht_with_c INTERFACE opendht)

    # ── Patch the opendht Rust crate ─────────────────────────────────────────
    # The opendht Rust crate has a type mismatch in crypto.rs (i8 vs u8) that
    # prevents it from compiling with current rustc.  We apply a patch at
    # configure time.  `patch --forward` succeeds on first apply and silently
    # skips if the patch is already applied (idempotent).
    find_program(PATCH_EXECUTABLE patch REQUIRED)
    set(OPENDHT_RUST_CRYPTO_PATCH
        "${CMAKE_SOURCE_DIR}/patches/opendht-rust-crypto-type-fix.patch")
    set(OPENDHT_RUST_CRYPTO_SRC
        "${OPENDHT_SUBMODULE_DIR}/rust/src/crypto.rs")
    execute_process(
        COMMAND "${PATCH_EXECUTABLE}" --forward --reject-file=- -p0
                "${OPENDHT_RUST_CRYPTO_SRC}"
                "${OPENDHT_RUST_CRYPTO_PATCH}"
        RESULT_VARIABLE PATCH_RESULT
        OUTPUT_QUIET
        ERROR_QUIET
    )
    # result 0 = applied cleanly, 1 = already applied (both are fine).
    if(PATCH_RESULT GREATER 1)
        message(WARNING "opendht Rust crypto patch failed (result ${PATCH_RESULT}) — "
                        "cargo build may fail. Check patches/opendht-rust-crypto-type-fix.patch")
    else()
        message(STATUS "opendht Rust crypto patch applied (or already present).")
    endif()

    add_compile_definitions(SNAPCLONE_OPENDHT_BACKEND)

    # Pass opendht build paths to cargo so build.rs can link libopendht.a.
    # CMAKE_BINARY_DIR/opendht_build is where add_subdirectory puts the output.
    set(OPENDHT_LIB_DIR     "${CMAKE_BINARY_DIR}/opendht_build")
    set(OPENDHT_INCLUDE_DIR "${OPENDHT_SUBMODULE_DIR}/include")
    message(STATUS "OpenDHT backend : ON  (submodule at ${OPENDHT_SUBMODULE_DIR})")
    message(STATUS "  System deps: apk add gnutls-dev nettle-dev argon2-dev msgpack-cxx-dev")

else()
    message(STATUS "DHT backend     : OFF (LAN gossip/WebSocket only)")
    message(STATUS "  Enable Jami:    cmake -DSNAPCLONE_JAMI_BACKEND=ON ...")
    message(STATUS "  Enable OpenDHT: cmake -DSNAPCLONE_OPENDHT_BACKEND=ON ...")
endif()

# ─── Rust Crypto Backend ──────────────────────────────────────────────────────
# When ON, CMake drives `cargo build` and links the resulting static library.
# The C header is generated by cbindgen in the crate's build.rs and placed in
# include/crypto/snapclone_crypto_ffi.h.
if(SNAPCLONE_RUST_CRYPTO)
    find_program(CARGO_EXE cargo REQUIRED
        HINTS "$ENV{HOME}/.cargo/bin"
        DOC "Rust Cargo package manager")

    set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_CRATE_DIR     "${RUST_WORKSPACE_DIR}/snapclone-crypto")
    set(RUST_TARGET_DIR "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_LIB "${RUST_TARGET_DIR}/release/libsnapclone_crypto.a")

    # Custom target so `cmake --build` drives cargo automatically.
    add_custom_target(snapclone_rust_crypto_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-crypto
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_LIB}"
        COMMENT    "Building Rust snapclone-crypto crate…"
        VERBATIM
    )

    # Create an IMPORTED library so snapclone_crypto can link against it.
    add_library(snapclone_rust_crypto_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_crypto_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_LIB}"
    )
    add_dependencies(snapclone_rust_crypto_lib snapclone_rust_crypto_build)

    add_compile_definitions(SNAPCLONE_RUST_CRYPTO)
    message(STATUS "Rust crypto     : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_LIB}")
else()
    message(STATUS "Rust crypto     : OFF (Qt HMAC-SHA256 placeholder)")
    message(STATUS "  Enable with   : cmake -DSNAPCLONE_RUST_CRYPTO=ON ...")
endif()

# ─── Rust Saliency Backend ────────────────────────────────────────────────────
# Same pattern as crypto. Provides classical (non-ML) salient region detection
# for the Wayfinder avatars — picks the most visually-interesting region of a
# snap rather than blindly center-cropping.
option(SNAPCLONE_RUST_SALIENCY "Build Rust saliency crate for smart avatar cropping" ON)
if(SNAPCLONE_RUST_SALIENCY)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR    "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_SALIENCY_LIB  "${RUST_TARGET_DIR}/release/libsnapclone_saliency.a")

    add_custom_target(snapclone_rust_saliency_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-saliency
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_SALIENCY_LIB}"
        COMMENT    "Building Rust snapclone-saliency crate…"
        VERBATIM
    )

    add_library(snapclone_rust_saliency_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_saliency_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_SALIENCY_LIB}"
    )
    add_dependencies(snapclone_rust_saliency_lib snapclone_rust_saliency_build)

    add_compile_definitions(SNAPCLONE_RUST_SALIENCY)
    message(STATUS "Rust saliency   : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_SALIENCY_LIB}")
else()
    message(STATUS "Rust saliency   : OFF (avatars will use center crop)")
endif()

# ─── Rust Group (sealed/signed group entries — sc_group_* FFI) ────────────────
# The security-critical group-message operations (seal+sign an outgoing entry,
# verify+open an incoming one — verify-before-decrypt) live in the tested Rust
# snapclone-group crate. C++ links this staticlib and calls the stateless C ABI
# in include/group/snapclone_group_ffi.h. See docs/GROUP_AND_SERVER_DESIGN.md.
option(SNAPCLONE_RUST_GROUP "Build Rust group crate for sealed group messaging" ON)
if(SNAPCLONE_RUST_GROUP)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR    "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_GROUP_LIB     "${RUST_TARGET_DIR}/release/libsnapclone_group.a")

    add_custom_target(snapclone_rust_group_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-group
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_GROUP_LIB}"
        COMMENT    "Building Rust snapclone-group crate…"
        VERBATIM
    )

    add_library(snapclone_rust_group_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_group_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_GROUP_LIB}"
    )
    add_dependencies(snapclone_rust_group_lib snapclone_rust_group_build)

    add_compile_definitions(SNAPCLONE_RUST_GROUP)
    message(STATUS "Rust group      : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_GROUP_LIB}")
else()
    message(STATUS "Rust group      : OFF (group messaging unavailable)")
endif()

# ─── Rust CoreSched (efficiency-core placement for battery) ───────────────────
# Classifies E/P cores from sysfs and decides which background thread classes to
# demote to efficiency cores (affinity/uclamp/nice). C++ applies the returned
# placement with the syscall. See docs/CORE_SCHEDULING.md.
option(SNAPCLONE_RUST_CORESCHED "Build Rust core-scheduling crate for battery savings" ON)
if(SNAPCLONE_RUST_CORESCHED)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR  "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR     "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_CORESCHED_LIB  "${RUST_TARGET_DIR}/release/libsnapclone_coresched.a")

    add_custom_target(snapclone_rust_coresched_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-coresched
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_CORESCHED_LIB}"
        COMMENT    "Building Rust snapclone-coresched crate…"
        VERBATIM
    )

    add_library(snapclone_rust_coresched_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_coresched_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_CORESCHED_LIB}"
    )
    add_dependencies(snapclone_rust_coresched_lib snapclone_rust_coresched_build)

    add_compile_definitions(SNAPCLONE_RUST_CORESCHED)
    message(STATUS "Rust coresched  : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_CORESCHED_LIB}")
else()
    message(STATUS "Rust coresched  : OFF (no E-core thread placement)")
endif()

# ─── Rust Menu + Mailbox (unified-mailbox dynamic submenus — sc_menu_* / ──────
#     sc_reqmb_* FFI) ─────────────────────────────────────────────────────────
# The unified mailbox's dynamic submenus are driven by two tested Rust crates:
#   • snapclone-menu    — given an entity kind + state, which actions to show
#                         (include/menu/snapclone_menu_ffi.h, sc_menu_*).
#   • snapclone-mailbox — the connection-request approval lifecycle
#                         (include/mailbox/snapclone_requests_ffi.h, sc_reqmb_*).
# Both are consumed by snapclone_ui (InboxWidget), so they link there. Always on:
# the mailbox is core functionality, not an optional backend. Same IMPORTED-
# staticlib pattern as the crates above; cargo builds them into the shared
# rust-target dir.
if(NOT CARGO_EXE)
    find_program(CARGO_EXE cargo REQUIRED
        HINTS "$ENV{HOME}/.cargo/bin"
        DOC "Rust Cargo package manager")
endif()

set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
set(RUST_TARGET_DIR    "${CMAKE_BINARY_DIR}/rust-target")
set(RUST_MENU_LIB      "${RUST_TARGET_DIR}/release/libsnapclone_menu.a")
set(RUST_MAILBOX_LIB   "${RUST_TARGET_DIR}/release/libsnapclone_mailbox.a")

add_custom_target(snapclone_rust_menu_build ALL
    COMMAND "${CARGO_EXE}" build --release
            --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
            --package       snapclone-menu
            --target-dir    "${RUST_TARGET_DIR}"
    BYPRODUCTS "${RUST_MENU_LIB}"
    COMMENT    "Building Rust snapclone-menu crate…"
    VERBATIM
)
add_library(snapclone_rust_menu_lib STATIC IMPORTED GLOBAL)
set_target_properties(snapclone_rust_menu_lib PROPERTIES
    IMPORTED_LOCATION "${RUST_MENU_LIB}"
)
add_dependencies(snapclone_rust_menu_lib snapclone_rust_menu_build)

add_custom_target(snapclone_rust_mailbox_build ALL
    COMMAND "${CARGO_EXE}" build --release
            --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
            --package       snapclone-mailbox
            --target-dir    "${RUST_TARGET_DIR}"
    BYPRODUCTS "${RUST_MAILBOX_LIB}"
    COMMENT    "Building Rust snapclone-mailbox crate…"
    VERBATIM
)
add_library(snapclone_rust_mailbox_lib STATIC IMPORTED GLOBAL)
set_target_properties(snapclone_rust_mailbox_lib PROPERTIES
    IMPORTED_LOCATION "${RUST_MAILBOX_LIB}"
)
add_dependencies(snapclone_rust_mailbox_lib snapclone_rust_mailbox_build)

message(STATUS "Rust menu+mailbox: ON  (${CARGO_EXE})")
message(STATUS "  Menu output   : ${RUST_MENU_LIB}")
message(STATUS "  Mailbox output: ${RUST_MAILBOX_LIB}")


# ─── Rust Audio (VAD / speech-likelihood) ─────────────────────────────────────
# Classical voice-activity detection for the accessibility speech-boost
# feature. See docs/TODO.md "Audio normalization + speech-boost". Pure-Rust,
# no deps; cheap enough to run per-frame during playback.
option(SNAPCLONE_RUST_AUDIO "Build Rust audio crate for VAD / speech-boost" ON)
if(SNAPCLONE_RUST_AUDIO)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR    "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_AUDIO_LIB     "${RUST_TARGET_DIR}/release/libsnapclone_audio.a")

    # Opus voice codec lives behind the audio crate's `opus` Cargo feature, which
    # links libopus. OFF by default so the build doesn't require libopus; turn ON
    # (and install libopus-dev / opus-dev) for the real voice codec.
    option(SNAPCLONE_OPUS_CODEC "Build the Opus voice codec (needs libopus)" OFF)
    set(SNAPCLONE_AUDIO_FEATURES "")
    if(SNAPCLONE_OPUS_CODEC)
        set(SNAPCLONE_AUDIO_FEATURES --features opus)
        add_compile_definitions(SNAPCLONE_OPUS_CODEC)   # expose to C++ (#ifdef)
    endif()

    add_custom_target(snapclone_rust_audio_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-audio
                ${SNAPCLONE_AUDIO_FEATURES}
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_AUDIO_LIB}"
        COMMENT    "Building Rust snapclone-audio crate…"
        VERBATIM
    )

    add_library(snapclone_rust_audio_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_audio_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_AUDIO_LIB}"
    )
    add_dependencies(snapclone_rust_audio_lib snapclone_rust_audio_build)

    if(SNAPCLONE_OPUS_CODEC)
        # audiopus (inside libsnapclone_audio.a) references the system Opus
        # library, but Cargo does NOT fold native deps into a staticlib — so the
        # final C++ executable must ALSO link libopus, or ld fails with
        # "undefined reference to opus_* / DSO missing from command line".
        # Attach it to the imported audio target's link interface so consumers
        # (snapclone_core/_ui → the SnapClone executable) pull it onto the link.
        find_package(PkgConfig)
        if(PkgConfig_FOUND)
            pkg_check_modules(OPUS IMPORTED_TARGET GLOBAL opus)
        endif()
        if(TARGET PkgConfig::OPUS)
            set_property(TARGET snapclone_rust_audio_lib APPEND
                PROPERTY INTERFACE_LINK_LIBRARIES PkgConfig::OPUS)
            message(STATUS "  Opus codec    : ON  (libopus ${OPUS_VERSION} via pkg-config)")
        else()
            find_library(OPUS_LIBRARY NAMES opus)
            if(OPUS_LIBRARY)
                set_property(TARGET snapclone_rust_audio_lib APPEND
                    PROPERTY INTERFACE_LINK_LIBRARIES "${OPUS_LIBRARY}")
                message(STATUS "  Opus codec    : ON  (libopus at ${OPUS_LIBRARY})")
            else()
                message(FATAL_ERROR
                    "SNAPCLONE_OPUS_CODEC=ON but libopus was not found for linking. "
                    "Install the Opus development package: apk add opus-dev "
                    "(Alpine/postmarketOS), apt install libopus-dev (Debian/Ubuntu), "
                    "or dnf install opus-devel (Fedora).")
            endif()
        endif()
    endif()

    add_compile_definitions(SNAPCLONE_RUST_AUDIO)
    message(STATUS "Rust audio      : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_AUDIO_LIB}")
else()
    message(STATUS "Rust audio      : OFF (speech boost will be static EQ only)")
endif()

# ─── Rust MediaShare (shared-conversation media dedup + multiway keyshare) ─────
# Content-addressed chunking so group images/videos aren't re-sent when members
# already hold the bytes, plus multiway key distribution (one content key wrapped
# per member). Pure-Rust + x25519/chacha; needs a recent rustc for those crates.
# The C++ media sender/receiver (to come) drive this; the codec
# (snapclone-vqcodec) and the transport are separate seams.
option(SNAPCLONE_RUST_MEDIASHARE "Build Rust mediashare crate (group media dedup + keyshare)" ON)
if(SNAPCLONE_RUST_MEDIASHARE)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR  "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR     "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_MEDIASHARE_LIB "${RUST_TARGET_DIR}/release/libsnapclone_mediashare.a")

    add_custom_target(snapclone_rust_mediashare_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-mediashare
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_MEDIASHARE_LIB}"
        COMMENT    "Building Rust snapclone-mediashare crate…"
        VERBATIM
    )

    add_library(snapclone_rust_mediashare_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_mediashare_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_MEDIASHARE_LIB}"
    )
    add_dependencies(snapclone_rust_mediashare_lib snapclone_rust_mediashare_build)

    add_compile_definitions(SNAPCLONE_RUST_MEDIASHARE)
    message(STATUS "Rust mediashare : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_MEDIASHARE_LIB}")
else()
    message(STATUS "Rust mediashare : OFF (group media dedup disabled)")
endif()

# ─── Rust Proximity (BLE RSSI ranging + bearing) ──────────────────────────────
# RSSI ranging + gradient-following bearing for the BLE peer pointing /
# triangulation feature. See docs/TODO.md "Bluetooth / ad-hoc pointing
# triangulation". Pure-Rust, no deps. The C++ ProximitySource (Qt/BlueZ)
# feeds RSSI + compass heading; this does the math.
option(SNAPCLONE_RUST_PROXIMITY "Build Rust proximity crate for BLE pointing" ON)
if(SNAPCLONE_RUST_PROXIMITY)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR    "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_PROXIMITY_LIB "${RUST_TARGET_DIR}/release/libsnapclone_proximity.a")

    add_custom_target(snapclone_rust_proximity_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-proximity
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_PROXIMITY_LIB}"
        COMMENT    "Building Rust snapclone-proximity crate…"
        VERBATIM
    )

    add_library(snapclone_rust_proximity_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_proximity_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_PROXIMITY_LIB}"
    )
    add_dependencies(snapclone_rust_proximity_lib snapclone_rust_proximity_build)

    add_compile_definitions(SNAPCLONE_RUST_PROXIMITY)
    message(STATUS "Rust proximity  : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_PROXIMITY_LIB}")
else()
    message(STATUS "Rust proximity  : OFF (BLE pointing unavailable)")
endif()

# ─── Rust Face Backend ────────────────────────────────────────────────────────
# Phase 1 of the face-aware bandwidth optimization (docs/FACE_BANDWIDTH.md).
# Builds the snapclone-face crate as a staticlib exposing the FFI declared
# in include/face/snapclone_face_ffi.h. C++ code links against it via the
# IMPORTED target snapclone_rust_face_lib.
#
# Default OFF for now — the FFI is wired (this turn) but no C++ callers
# exist yet. Flipping to ON pulls in the crate, the staticlib, and the
# tract-onnx dep tree (~6 MB compiled). Will flip to default-ON in a
# later turn when the camera/inbox wiring lands.
option(SNAPCLONE_RUST_FACE "Build Rust face crate for face library + detection" OFF)
if(SNAPCLONE_RUST_FACE)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR    "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_FACE_LIB      "${RUST_TARGET_DIR}/release/libsnapclone_face.a")

    add_custom_target(snapclone_rust_face_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-face
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_FACE_LIB}"
        COMMENT    "Building Rust snapclone-face crate…"
        VERBATIM
    )

    add_library(snapclone_rust_face_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_face_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_FACE_LIB}"
    )
    add_dependencies(snapclone_rust_face_lib snapclone_rust_face_build)

    add_compile_definitions(SNAPCLONE_RUST_FACE)
    message(STATUS "Rust face       : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_FACE_LIB}")
else()
    message(STATUS "Rust face       : OFF (no face library — full image always sent)")
    message(STATUS "  Enable with   : cmake -DSNAPCLONE_RUST_FACE=ON ...")
endif()

# ─── Rust VQ codec (shared-codebook image compression + C FFI) ────────────────
# Off by default — no C++ caller wires it into the media path yet. When ON it
# builds libsnapclone_vqcodec.a (extern "C" surface in
# rust/snapclone-vqcodec/src/ffi.rs; header at
# include/vqcodec/snapclone_vqcodec_ffi.h) and links it into snapclone_ui,
# ready to compress / expand snaps once the send/receive path calls it.
option(SNAPCLONE_RUST_VQCODEC "Build Rust VQ image-codec crate + C FFI" OFF)
if(SNAPCLONE_RUST_VQCODEC)
    if(NOT CARGO_EXE)
        find_program(CARGO_EXE cargo REQUIRED
            HINTS "$ENV{HOME}/.cargo/bin"
            DOC "Rust Cargo package manager")
    endif()

    set(RUST_WORKSPACE_DIR "${CMAKE_SOURCE_DIR}/rust")
    set(RUST_TARGET_DIR    "${CMAKE_BINARY_DIR}/rust-target")
    set(RUST_VQCODEC_LIB   "${RUST_TARGET_DIR}/release/libsnapclone_vqcodec.a")

    add_custom_target(snapclone_rust_vqcodec_build ALL
        COMMAND "${CARGO_EXE}" build --release
                --manifest-path "${RUST_WORKSPACE_DIR}/Cargo.toml"
                --package       snapclone-vqcodec
                --target-dir    "${RUST_TARGET_DIR}"
        BYPRODUCTS "${RUST_VQCODEC_LIB}"
        COMMENT    "Building Rust snapclone-vqcodec crate…"
        VERBATIM
    )

    add_library(snapclone_rust_vqcodec_lib STATIC IMPORTED GLOBAL)
    set_target_properties(snapclone_rust_vqcodec_lib PROPERTIES
        IMPORTED_LOCATION "${RUST_VQCODEC_LIB}"
    )
    add_dependencies(snapclone_rust_vqcodec_lib snapclone_rust_vqcodec_build)

    add_compile_definitions(SNAPCLONE_RUST_VQCODEC)
    message(STATUS "Rust vqcodec    : ON  (${CARGO_EXE})")
    message(STATUS "  Output        : ${RUST_VQCODEC_LIB}")
else()
    message(STATUS "Rust vqcodec    : OFF (no on-device VQ image compression)")
    message(STATUS "  Enable with   : cmake -DSNAPCLONE_RUST_VQCODEC=ON ...")
endif()

# ─── libcamera backend (direct C++ capture) ──────────────────────────────────
# Preferred camera backend on mobile Linux (postmarketOS / Fairphone 5), where
# the stack is libcamera-native and gst-launch through the bridge is flaky.
# Optional + graceful: default ON, but if libcamera-dev isn't installed we
# silently disable it (camera falls back to the gst-launch / Qt backends) rather
# than failing the build — so desktop builds without libcamera headers work.
option(SNAPCLONE_LIBCAMERA "Use the direct libcamera C++ camera backend (preferred on mobile Linux)" ON)
if(SNAPCLONE_LIBCAMERA)
    find_package(PkgConfig)
    if(PkgConfig_FOUND)
        pkg_check_modules(LIBCAMERA IMPORTED_TARGET GLOBAL libcamera)
    endif()
    if(LIBCAMERA_FOUND)
        add_compile_definitions(SNAPCLONE_LIBCAMERA)
        message(STATUS "libcamera      : ON  (${LIBCAMERA_VERSION}) — direct C++ backend, gst-launch demoted")
    else()
        set(SNAPCLONE_LIBCAMERA OFF)
        message(STATUS "libcamera      : requested but not found — skipping (camera falls back to gst-launch/Qt)")
        message(STATUS "  Install with : apt-get install libcamera-dev   (or your distro's package)")
    endif()
endif()

# ─── Source Subdirectories ────────────────────────────────────────────────────
# IMPORTANT: add_subdirectory MUST come after all add_compile_definitions calls
# above, otherwise the child targets will not see the feature flags.
add_subdirectory(src)

if(SNAPCLONE_BUILD_TESTS)
    # Re-enable BUILD_TESTING for our own tests (was forced OFF for opendht).
    set(BUILD_TESTING ON CACHE BOOL "" FORCE)
    enable_testing()
    add_subdirectory(tests)
endif()

if(SNAPCLONE_BUILD_TOOLS)
    add_subdirectory(tools)
endif()
