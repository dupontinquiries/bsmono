add_library(snapclone_debug STATIC
    DebugFlags.cpp
    # Header listed so AUTOMOC sees Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/debug/DebugFlags.hpp
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
