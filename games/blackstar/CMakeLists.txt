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
  ${EDIR}/voice.c
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
  ${EDIR}/net_transport.c
  ${EDIR}/bsgames_handoff.c
  ${EDIR}/mapfsm.c
  ${EDIR}/keybind.c
  ${EDIR}/combat.c
  ${EDIR}/combat_json.c
  ${EDIR}/commander_ai.c
  ${EDIR}/hive_ai.c
  ${EDIR}/enemy.c
  ${EDIR}/behavior.c
  ${EDIR}/arena.c
  ${EDIR}/element.c
  ${EDIR}/menu.c
  ${EDIR}/robot_parts.c
  ${EDIR}/campaign.c
  ${EDIR}/weapon.c
  ${EDIR}/encounter.c
  ${EDIR}/detail.c
  ${EDIR}/vehicle.c
  ${EDIR}/ability.c
  ${EDIR}/weaponmod.c
  ${EDIR}/turret.c
  ${EDIR}/mapgen.c
  ${EDIR}/terrain_step.c
  ${EDIR}/pclass.c
  ${EDIR}/chassis.c
  ${EDIR}/signature.c
  ${EDIR}/warp_wave.c
  ${EDIR}/tsunami.c
  ${EDIR}/arc.c
  ${EDIR}/cyclone.c
  ${EDIR}/thorns.c
  ${EDIR}/photon.c
  ${EDIR}/matrix.c
  ${EDIR}/spearcon.c
  ${EDIR}/resonance.c
  ${EDIR}/corrupt.c
  ${EDIR}/graviton.c
  ${EDIR}/siphon.c
  ${EDIR}/truce.c
  ${EDIR}/containment.c
  ${EDIR}/absorb.c
  ${EDIR}/ally.c
  ${EDIR}/feel.c
  ${EDIR}/material.c
  ${EDIR}/lore.c
  ${EDIR}/waves.c
  ${EDIR}/gates.c
  ${EDIR}/pickup.c
  ${EDIR}/vfx.c
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
# Optional Vulkan backend scaffold (-DUSE_VULKAN=ON)
# HONEST SCOPE: brings Vulkan up through a clearing render pass (instance ->
# device -> swapchain -> render pass -> present). This is the foundation of
# the GL->Vulkan migration, not a full port of the deferred pipeline. No
# hardware ray tracing (Mesa/Asahi aarch64 target doesn't expose it).
# Uses vendored Khronos headers in third_party/ and links the Vulkan loader.
# ---------------------------------------------------------------------------
option(USE_VULKAN "Build the Vulkan renderer scaffold (bring-up only)" OFF)
if(USE_VULKAN)
  find_library(VULKAN_LIB NAMES vulkan vulkan-1)
  if(NOT VULKAN_LIB)
    message(FATAL_ERROR "USE_VULKAN=ON but no Vulkan loader (libvulkan) found")
  endif()
  message(STATUS "Vulkan scaffold enabled, loader: ${VULKAN_LIB}")
  target_sources(exengine PRIVATE ${EDIR}/vk_backend.c)
  target_include_directories(exengine PUBLIC ${CMAKE_SOURCE_DIR}/third_party/vulkan)
  target_link_libraries(exengine PUBLIC ${VULKAN_LIB})
  target_compile_definitions(exengine PUBLIC EX_HAVE_VULKAN=1)

  # standalone probe: creates an instance and enumerates GPUs (no window needed)
  add_executable(vk_probe ${CMAKE_SOURCE_DIR}/tests/vk_probe.c)
  target_include_directories(vk_probe PRIVATE ${CMAKE_SOURCE_DIR}/third_party/vulkan ${EDIR})
  target_link_libraries(vk_probe PRIVATE exengine ${VULKAN_LIB})
endif()

# ---------------------------------------------------------------------------
# game executable
# ---------------------------------------------------------------------------
add_executable(game
  ${ENGINE_SRC}/main.c
  ${ENGINE_SRC}/game.c
)
target_link_libraries(game PRIVATE exengine)

# ---------------------------------------------------------------------------
# data.ex asset archive
# The engine mounts a PhysFS archive named "data.ex" (see engine.h /
# EX_DATA_FILE) relative to the working directory. The original Makefile
# built it in its `files:` target by zipping two data trees into one archive:
#   - exengine/data/  -> shaders   (entries become data/shaders/*.glsl)
#   - data/           -> assets    (entries become data/conf.ini, textures, iqm, ...)
# We reproduce that here so `game` has its assets and doesn't segfault on a
# missing shader. The archive is written next to the game binary in the
# build dir; run the game from there (cd build && ./game) OR from any dir
# that contains a copy of data.ex.
# ---------------------------------------------------------------------------
find_program(ZIP_EXECUTABLE zip)
if(NOT ZIP_EXECUTABLE)
  message(WARNING "'zip' not found -- data.ex will not be built, game will not run. Install zip.")
else()
  set(DATA_EX ${CMAKE_BINARY_DIR}/data.ex)
  # A custom TARGET (not an OUTPUT command) always runs, so data.ex is repacked
  # on every build. This is deliberate: an OUTPUT command with no explicit
  # DEPENDS is considered up-to-date once data.ex exists, which silently drops
  # any data files added later (e.g. a new arena.iqm). Repacking is cheap.
  add_custom_target(data_archive ALL
    # rebuild cleanly each time so stale/removed entries don't linger
    COMMAND ${CMAKE_COMMAND} -E rm -f ${DATA_EX}
    # shaders: run zip from inside exengine/ so archive paths are data/shaders/...
    COMMAND ${CMAKE_COMMAND} -E chdir ${EDIR}
            ${ZIP_EXECUTABLE} -r -q ${DATA_EX} data
    # assets: run zip from inside engine_src/ so archive paths are data/...
    COMMAND ${CMAKE_COMMAND} -E chdir ${ENGINE_SRC}
            ${ZIP_EXECUTABLE} -ur -q ${DATA_EX} data
    COMMENT "Building data.ex asset archive (shaders + assets)"
    VERBATIM
  )
  add_dependencies(game data_archive)
endif()

# ---------------------------------------------------------------------------
# module tests -- these do NOT need SDL/GL, only the pure-logic modules,
# so they build and run headless (perfect for CI on Mesa/Asahi aarch64 too)
# ---------------------------------------------------------------------------
enable_testing()

add_library(cybertron_modules STATIC ${CYBERTRON_MODULE_SRC}
  ${EDIR}/math/collision.c ${EDIR}/math/octree.c)
target_include_directories(cybertron_modules PUBLIC ${ENGINE_INCLUDES})
target_compile_options(cybertron_modules PRIVATE -Wno-unused)

foreach(t example_usage test_transform test_netcode test_gameplay test_element test_menu test_vfx test_gait test_weapon test_encounter test_pickup test_custom test_mapgen test_waves harness_step test_class test_feel test_material test_movesim test_genome test_signature test_climb test_warp_wave test_tsunami test_arc test_cyclone test_thorns test_photon test_matrix test_spearcon test_resonance test_corrupt test_graviton test_siphon test_truce test_containment test_absorb test_ally test_content_gen test_detail test_behavior test_combat test_weaponmod test_ability test_vehicle test_turret test_wheels test_enemy test_combat_json test_commander_ai test_hive_ai test_bodykind test_gates test_handoff)
  add_executable(${t} ${CMAKE_SOURCE_DIR}/tests/${t}.c)
  target_link_libraries(${t} PRIVATE cybertron_modules m)
  target_compile_options(${t} PRIVATE -UNDEBUG)   # tests must keep their assert()s even in Release
  add_test(NAME ${t} COMMAND ${t})
endforeach()

# test_transport takes a "server"/"client" arg -> not a plain ctest;
# build the binary and run two copies manually (see README).
add_executable(test_transport ${CMAKE_SOURCE_DIR}/tests/test_transport.c)
target_link_libraries(test_transport PRIVATE cybertron_modules m)
target_compile_options(test_transport PRIVATE -UNDEBUG)

# ---------------------------------------------------------------------------
# Rust FFI bridge (-DUSE_RUST_FSM=ON)
# Links the real early-fps fsm.rs gating (rust_fsm crate) over a C ABI, so
# the C engine can defer map-feature placement decisions to Rust. The crate
# is built by cargo; we import the resulting static lib. Requires cargo.
# ---------------------------------------------------------------------------
option(USE_RUST_FSM "Link the Rust fsm.rs gating via C FFI" OFF)

add_executable(test_mapfsm ${CMAKE_SOURCE_DIR}/tests/test_mapfsm.c)
target_link_libraries(test_mapfsm PRIVATE cybertron_modules m)
target_compile_options(test_mapfsm PRIVATE -UNDEBUG)

if(USE_RUST_FSM)
  find_program(CARGO_EXECUTABLE cargo)
  if(NOT CARGO_EXECUTABLE)
    message(FATAL_ERROR "USE_RUST_FSM=ON but cargo not found")
  endif()
  set(RUST_CRATE ${CMAKE_SOURCE_DIR}/rust_fsm)
  set(RUST_LIB ${RUST_CRATE}/target/release/librust_fsm.a)
  add_custom_command(
    OUTPUT ${RUST_LIB}
    COMMAND ${CARGO_EXECUTABLE} build --release --offline
    WORKING_DIRECTORY ${RUST_CRATE}
    COMMENT "Building Rust fsm.rs FFI bridge (cargo)"
    VERBATIM
  )
  add_custom_target(rust_fsm_build DEPENDS ${RUST_LIB})
  add_dependencies(test_mapfsm rust_fsm_build)
  target_compile_definitions(test_mapfsm PRIVATE EX_HAVE_RUST_FSM=1)
  # link the Rust staticlib + its std deps
  target_link_libraries(test_mapfsm PRIVATE ${RUST_LIB} pthread dl)
  message(STATUS "Rust FSM FFI enabled")
endif()
add_test(NAME test_mapfsm COMMAND test_mapfsm)
# data-driven content graph: validate the JSON relations against meshes + C enums
find_package(Python3 COMPONENTS Interpreter QUIET)
if(Python3_FOUND)
  add_test(NAME content_validate
           COMMAND ${Python3_EXECUTABLE} ${CMAKE_SOURCE_DIR}/tools/content_build.py
                   --root ${CMAKE_SOURCE_DIR} --quiet)
endif()
if(Python3_FOUND)
  add_test(NAME lint_dead_fields
           COMMAND ${Python3_EXECUTABLE} ${CMAKE_SOURCE_DIR}/tools/lint_dead_fields.py
                   --root ${CMAKE_SOURCE_DIR})
endif()
# Rust twin of lint_dead_fields (rust/contentlint) — first port under
# maintenance/RUST_PORT_PLAN.md. Byte-identical stdout + exit semantics, proven on both
# the pass and fail paths against the Python. Runs wherever cargo exists — including
# hosts with no Python3, which is the gap that motivated the port (these content gates
# silently drop out of the suite there). The Python stays until the twin has run green
# on every suite host; retire it by deleting the block above, nothing else.
find_program(CARGO_EXECUTABLE cargo QUIET)
if(CARGO_EXECUTABLE)
  add_test(NAME lint_dead_fields_rs
           COMMAND ${CARGO_EXECUTABLE} run --release --quiet
                   --manifest-path ${CMAKE_SOURCE_DIR}/rust/contentlint/Cargo.toml
                   --bin lint_dead_fields -- --root ${CMAKE_SOURCE_DIR})
endif()
if(Python3_FOUND)
  add_test(NAME silhouette
           COMMAND ${Python3_EXECUTABLE} ${CMAKE_SOURCE_DIR}/tools/test_silhouette.py
           WORKING_DIRECTORY ${CMAKE_SOURCE_DIR})
endif()
if(Python3_FOUND)
  add_test(NAME map_score
           COMMAND ${Python3_EXECUTABLE} ${CMAKE_SOURCE_DIR}/tools/map_score.py --check
           WORKING_DIRECTORY ${CMAKE_SOURCE_DIR})
endif()
if(Python3_FOUND)
  add_test(NAME openworld
           COMMAND ${Python3_EXECUTABLE} ${CMAKE_SOURCE_DIR}/tools/test_openworld.py
           WORKING_DIRECTORY ${CMAKE_SOURCE_DIR})
endif()
if(Python3_FOUND)
  add_test(NAME gltf_roundtrip
           COMMAND ${Python3_EXECUTABLE} ${CMAKE_SOURCE_DIR}/tools/test_gltf.py)
endif()
if(Python3_FOUND)
  add_test(NAME model_tuning
           COMMAND ${Python3_EXECUTABLE} ${CMAKE_SOURCE_DIR}/tools/test_tuning.py
                   --root ${CMAKE_SOURCE_DIR})
endif()

add_executable(test_tangents ${CMAKE_SOURCE_DIR}/tests/test_tangents.c)
target_include_directories(test_tangents PRIVATE ${CMAKE_SOURCE_DIR}/engine_src/exengine)
target_link_libraries(test_tangents PRIVATE m)
target_compile_options(test_tangents PRIVATE -UNDEBUG)
add_test(NAME test_tangents COMMAND test_tangents ${CMAKE_SOURCE_DIR}/engine_src/data/pchest.iqm)

add_executable(test_arena_iqm ${CMAKE_SOURCE_DIR}/tests/test_arena_iqm.c)
target_link_libraries(test_arena_iqm PRIVATE m)
target_compile_options(test_arena_iqm PRIVATE -UNDEBUG)
add_test(NAME test_arena_iqm COMMAND test_arena_iqm ${CMAKE_SOURCE_DIR}/engine_src/data/arena.iqm)

# spine-grown character-part IQMs (tools/gen_spine.py) -- same headless format
# guarantee, plus the unit-slot normalization invariant.
add_executable(test_spine_iqm ${CMAKE_SOURCE_DIR}/tests/test_spine_iqm.c)
target_link_libraries(test_spine_iqm PRIVATE m)
target_compile_options(test_spine_iqm PRIVATE -UNDEBUG)
add_test(NAME test_spine_iqm COMMAND test_spine_iqm
  ${CMAKE_SOURCE_DIR}/engine_src/data/spine_wing_cannon.iqm
  ${CMAKE_SOURCE_DIR}/engine_src/data/spine_muscle.iqm
  ${CMAKE_SOURCE_DIR}/engine_src/data/spine_cable.iqm
  ${CMAKE_SOURCE_DIR}/engine_src/data/spine_crest.iqm)
