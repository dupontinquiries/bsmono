add_library(snapclone_storage STATIC
    StorageManager.cpp
    # Headers must be listed so AUTOMOC can find Q_OBJECT macros in include/
    ${PROJECT_SOURCE_DIR}/include/storage/StorageManager.hpp
)

target_include_directories(snapclone_storage
    PUBLIC
        ${PROJECT_SOURCE_DIR}/include
)

target_link_libraries(snapclone_storage
    PUBLIC
        snapclone_warnings
        Qt6::Core
        Qt6::Sql
)

set_target_properties(snapclone_storage PROPERTIES AUTOMOC ON)
