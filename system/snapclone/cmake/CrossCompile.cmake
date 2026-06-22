# cmake/CrossCompile.cmake — cross compilation toolchain for postmarketOS / Alpine aarch64
#
# Usage:
#   cmake -B build-arm \
#     -DCMAKE_TOOLCHAIN_FILE=cmake/CrossCompile.cmake \
#     -DTARGET_TRIPLE=aarch64-unknown-linux-musl
#
# Prerequisites on the build host:
#   apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
#   rustup target add aarch64-unknown-linux-musl

cmake_minimum_required(VERSION 3.20)

if(NOT DEFINED TARGET_TRIPLE)
    set(TARGET_TRIPLE "aarch64-unknown-linux-musl" CACHE STRING "Target triple")
endif()
message(STATUS "Cross compiling for: ${TARGET_TRIPLE}")

if(TARGET_TRIPLE MATCHES "^aarch64")
    set(CMAKE_SYSTEM_PROCESSOR "aarch64")
    set(CROSS_PREFIX           "aarch64-linux-gnu-")
    set(RUST_TARGET            "aarch64-unknown-linux-musl")
elseif(TARGET_TRIPLE MATCHES "^x86_64.*musl")
    set(CMAKE_SYSTEM_PROCESSOR "x86_64")
    set(CROSS_PREFIX           "x86_64-linux-musl-")
    set(RUST_TARGET            "x86_64-unknown-linux-musl")
else()
    message(FATAL_ERROR "Unsupported TARGET_TRIPLE: ${TARGET_TRIPLE}")
endif()

set(CMAKE_SYSTEM_NAME "Linux")
find_program(CMAKE_C_COMPILER   "${CROSS_PREFIX}gcc"   REQUIRED)
find_program(CMAKE_CXX_COMPILER "${CROSS_PREFIX}g++"   REQUIRED)
find_program(CMAKE_AR           "${CROSS_PREFIX}ar")
find_program(CMAKE_STRIP        "${CROSS_PREFIX}strip")

if(DEFINED CROSS_SYSROOT)
    set(CMAKE_SYSROOT "${CROSS_SYSROOT}")
    set(CMAKE_FIND_ROOT_PATH "${CROSS_SYSROOT}")
    set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
    set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
    set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
endif()

set(RUST_TARGET "${RUST_TARGET}" CACHE STRING "Rust cross target" FORCE)
set(CARGO_BUILD_FLAGS "--target=${RUST_TARGET}" "--release"
    CACHE STRING "cargo build flags for cross")

message(STATUS "Rust target: ${RUST_TARGET}")
add_link_options(-static-libgcc -static-libstdc++)
