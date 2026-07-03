cmake_minimum_required(VERSION 3.16)
project(cybertron_arena C)

# ---------------------------------------------------------------------------
# Cybertron: Arena of Gods
# CMake build that rolls the exengine engine into this codebase as a static
# library (libexengine), builds the game executable against it, and builds
# the standalone module tests (chunk / transform / netcode).
# ---------------------------------------------------------------------------

set(CMAKE_C_STANDARD 99)
set(CMAKE_C_STANDARD_REQUIRED ON)

if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

set(ENGINE_SRC ${CMAKE_SOURCE_DIR}/engine_src)
set(EDIR       ${ENGINE_SRC}/exengine)
set(LDIR       ${ENGINE_SRC}/lib)

# ---- include paths (mirror the original Makefile's -I set) ----
set(ENGINE_INCLUDES
  ${ENGINE_SRC}
  ${ENGINE_SRC}/inc
  ${LDIR}
  ${LDIR}/physfs
  ${LDIR}/MojoAL
  ${LDIR}/SDL2
  ${EDIR}
)

# ---------------------------------------------------------------------------
# Bundled third-party: PhysFS + MojoAL (compiled straight from the tree,
# exactly like the original Makefile did -- no system packages required)
# ---------------------------------------------------------------------------
file(GLOB PHYSFS_SRC ${LDIR}/physfs/*.c)
set(MOJOAL_SRC ${LDIR}/MojoAL/mojoal.c)

# ---------------------------------------------------------------------------
# Engine sources: everything under exengine/ EXCEPT our game-specific modules,
# which we list explicitly so the set is obvious and reviewable.
# ---------------------------------------------------------------------------
set(EXENGINE_CORE_SRC
  ${EDIR}/render/glad.c
  ${EDIR}/engine.c
  ${EDIR}/input/input.c
  ${EDIR}/math/collision.c
  ${EDIR}/math/octree.c
  ${EDIR}/render/window.c
  ${EDIR}/render/mesh.c
  ${EDIR}/render/camera.c
  ${EDIR}/render/texture.c
  ${EDIR}/render/pointlight.c
  ${EDIR}/render/scene.c
  ${EDIR}/render/model.c
  ${EDIR}/render/shader.c
  ${EDIR}/render/text.c
  ${EDIR}/render/msdf.c
  ${EDIR}/render/vga.c
  ${EDIR}/render/renderer.c
  ${EDIR}/sound/sound.c
  ${EDIR}/util/iqm.c
  ${EDIR}/util/entity.c
  ${EDIR}/util/cache.c
  ${EDIR}/util/ini.c
)

# ---- OUR new game modules, now first-class engine citizens ----
set(CYBERTRON_MODULE_SRC
  ${EDIR}/coll_chunk.c
  ${EDIR}/events.c
  ${EDIR}/transform.c
  ${EDIR}/netcode.c
)

# ---------------------------------------------------------------------------
# libexengine: the engine + bundled deps + our modules as one static lib
# ---------------------------------------------------------------------------
add_library(exengine STATIC
  ${EXENGINE_CORE_SRC}
  ${CYBERTRON_MODULE_SRC}
  ${PHYSFS_SRC}
  ${MOJOAL_SRC}
)
target_include_directories(exengine PUBLIC ${ENGINE_INCLUDES})
target_compile_options(exengine PRIVATE
  -Wno-unused -Wno-uninitialized -Wno-deprecated-declarations)
# MojoAL builds as a static AL implementation
target_compile_definitions(exengine PRIVATE AL_LIBTYPE_STATIC)

# ---- system libs the engine links against ----
# exengine uses glad, which vendors its own GL headers, so we only need the
# libGL *runtime* to link -- not the system GL dev headers. Find the lib
# directly to stay portable to Mesa (desktop) and Asahi (aarch64) where the
# GL/GLES vendor library is present but dev headers may not be.
find_library(GL_LIB NAMES GL GLESv2 OpenGL)
if(NOT GL_LIB)
  message(FATAL_ERROR "Could not find a GL runtime library (GL/GLESv2/OpenGL)")
endif()
message(STATUS "Using GL library: ${GL_LIB}")

# SDL2: prefer find_package, fall back to the bundled headers + system runtime
find_package(SDL2 QUIET)
if(SDL2_FOUND)
  target_link_libraries(exengine PUBLIC SDL2::SDL2)
else()
  message(STATUS "SDL2 cmake package not found; using bundled headers + system libSDL2")
  target_link_libraries(exengine PUBLIC SDL2)
endif()
target_link_libraries(exengine PUBLIC ${GL_LIB} m dl)

# ---------------------------------------------------------------------------
# game executable
# ---------------------------------------------------------------------------
add_executable(game
  ${ENGINE_SRC}/main.c
  ${ENGINE_SRC}/game.c
)
target_link_libraries(game PRIVATE exengine)

# ---------------------------------------------------------------------------
# module tests -- these do NOT need SDL/GL, only the pure-logic modules,
# so they build and run headless (perfect for CI on Mesa/Asahi aarch64 too)
# ---------------------------------------------------------------------------
enable_testing()

add_library(cybertron_modules STATIC ${CYBERTRON_MODULE_SRC}
  ${EDIR}/math/collision.c ${EDIR}/math/octree.c)
target_include_directories(cybertron_modules PUBLIC ${ENGINE_INCLUDES})
target_compile_options(cybertron_modules PRIVATE -Wno-unused)

foreach(t example_usage test_transform test_netcode)
  add_executable(${t} ${CMAKE_SOURCE_DIR}/tests/${t}.c)
  target_link_libraries(${t} PRIVATE cybertron_modules m)
  add_test(NAME ${t} COMMAND ${t})
endforeach()
