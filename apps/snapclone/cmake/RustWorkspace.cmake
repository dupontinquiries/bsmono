# cmake/RustWorkspace.cmake — builds the Rust workspace and exposes targets
#
# Include from root CMakeLists.txt:
#   include(cmake/RustWorkspace.cmake)
#   target_link_libraries(snapclone PRIVATE snapclone_rust_libs)

include(ExternalProject)

set(RUST_DIR "${CMAKE_SOURCE_DIR}/rust")
set(RUST_TARGET_DIR "${CMAKE_BINARY_DIR}/rust-target")

# Determine cargo flags (native or cross)
if(DEFINED RUST_TARGET)
    set(CARGO_FLAGS --target ${RUST_TARGET} --release)
    set(RUST_LIB_DIR "${RUST_TARGET_DIR}/${RUST_TARGET}/release")
else()
    set(CARGO_FLAGS --release)
    set(RUST_LIB_DIR "${RUST_TARGET_DIR}/release")
endif()

ExternalProject_Add(rust_workspace
    SOURCE_DIR      "${RUST_DIR}"
    BINARY_DIR      "${RUST_DIR}"
    BUILD_IN_SOURCE TRUE
    CONFIGURE_COMMAND ""
    BUILD_COMMAND
        cargo build
        ${CARGO_FLAGS}
        --target-dir "${RUST_TARGET_DIR}"
        --workspace
    INSTALL_COMMAND ""
    BUILD_BYPRODUCTS
        "${RUST_LIB_DIR}/libsnapclone_crypto.a"
        "${RUST_LIB_DIR}/libsnapclone_nat.a"
        "${RUST_LIB_DIR}/libsnapclone_peer.a"
    # Rebuild when any Rust source changes
    BUILD_ALWAYS TRUE
)

# Interface library so C++ targets can link against all Rust crates at once
add_library(snapclone_rust_libs INTERFACE)
add_dependencies(snapclone_rust_libs rust_workspace)

foreach(CRATE crypto nat peer proto storage identity)
    set(LIB "${RUST_LIB_DIR}/libsnapclone_${CRATE}.a")
    target_link_libraries(snapclone_rust_libs INTERFACE "${LIB}")
endforeach()
