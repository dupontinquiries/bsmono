add_library(snapclone_debug STATIC
    DebugFlags.cpp
    # Trace channels: the Qt-FREE policy core (parsing, rules, masks) plus the
    # thin Qt glue. The core is deliberately free of Qt so tests/test_TraceChannels
    # can build it with a plain compiler — no Qt, no display, no app.
    TraceChannels.cpp
    TraceControl.cpp
    # Headers listed so AUTOMOC sees Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/debug/DebugFlags.hpp
    ${PROJECT_SOURCE_DIR}/include/debug/TraceControl.hpp
)

target_include_directories(snapclone_debug
    PUBLIC
        ${PROJECT_SOURCE_DIR}/include
)

target_link_libraries(snapclone_debug
    PUBLIC
        snapclone_warnings
        Qt6::Core
)
