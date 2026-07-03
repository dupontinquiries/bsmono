# ─── tools/CMakeLists.txt ─────────────────────────────────────────────────────
# Standalone dev tools, built only when -DSNAPCLONE_BUILD_TOOLS=ON. These are not
# part of the app and link as little as possible so they're fast to iterate on.

# ringtone_workshop: render/audition the incoming-call ringtone. Compiles the
# shared RingtoneSynth directly (QtCore-only) rather than linking snapclone_ui,
# so a rebuild is near-instant.
add_executable(ringtone_workshop
    ringtone_workshop.cpp
    ${CMAKE_SOURCE_DIR}/src/ui/RingtoneSynth.cpp
)
target_include_directories(ringtone_workshop PRIVATE ${CMAKE_SOURCE_DIR}/include)
target_link_libraries(ringtone_workshop PRIVATE Qt6::Core)

# Optional live playback (--play). Enabled automatically if QtMultimedia is
# present; otherwise the tool still builds and just renders WAVs.
find_package(Qt6 QUIET COMPONENTS Multimedia)
if(Qt6Multimedia_FOUND)
    target_link_libraries(ringtone_workshop PRIVATE Qt6::Multimedia)
    target_compile_definitions(ringtone_workshop PRIVATE RINGTONE_WORKSHOP_PLAY)
    message(STATUS "ringtone_workshop: QtMultimedia found — --play enabled")
else()
    message(STATUS "ringtone_workshop: QtMultimedia not found — WAV render only")
endif()

# No Q_OBJECT in the tool, so AUTOMOC just slows configure down.
set_target_properties(ringtone_workshop PROPERTIES AUTOMOC OFF AUTOUIC OFF AUTORCC OFF)
