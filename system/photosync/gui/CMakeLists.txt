cmake_minimum_required(VERSION 3.16)

project(photosync-camera VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTORCC ON)

# Find Qt packages
find_package(Qt5 5.15 REQUIRED COMPONENTS
    Core
    Gui
    Qml
    Quick
    QuickControls2
    Multimedia
    Concurrent
    Positioning
    Network
)

# Find libcamera
find_package(PkgConfig REQUIRED)
pkg_check_modules(LIBCAMERA REQUIRED libcamera)

# Find other dependencies
pkg_check_modules(LIBDRM REQUIRED libdrm)
pkg_check_modules(MESA REQUIRED gbm egl)

# Sources
set(SOURCES
    src/main.cpp
    src/CameraManager.cpp
    src/PhotoProcessor.cpp
)

# Headers
set(HEADERS
    src/CameraManager.h
    src/PhotoProcessor.h
)

# QML resources
qt5_add_resources(QML_RESOURCES qml/qml.qrc)

# Create executable
add_executable(photosync-camera
    ${SOURCES}
    ${HEADERS}
    ${QML_RESOURCES}
)

# Include directories
target_include_directories(photosync-camera PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${LIBCAMERA_INCLUDE_DIRS}
    ${LIBDRM_INCLUDE_DIRS}
    ${MESA_INCLUDE_DIRS}
)

# Link libraries
target_link_libraries(photosync-camera PRIVATE
    Qt5::Core
    Qt5::Gui
    Qt5::Qml
    Qt5::Quick
    Qt5::QuickControls2
    Qt5::Multimedia
    Qt5::Concurrent
    Qt5::Positioning
    Qt5::Network
    ${LIBCAMERA_LIBRARIES}
    ${LIBDRM_LIBRARIES}
    ${MESA_LIBRARIES}
    pthread
)

# Compiler flags
target_compile_options(photosync-camera PRIVATE
    -Wall
    -Wextra
    -O3
    -march=native
    -mtune=native
    -flto
)

# Linker flags
set_target_properties(photosync-camera PROPERTIES
    LINK_FLAGS "-flto -Wl,-O1"
)

# Install
install(TARGETS photosync-camera
    RUNTIME DESTINATION bin
)

# Install desktop file
install(FILES
    ${CMAKE_CURRENT_SOURCE_DIR}/photosync-camera.desktop
    DESTINATION share/applications
)

# Install icon
install(FILES
    ${CMAKE_CURRENT_SOURCE_DIR}/photosync-camera.svg
    DESTINATION share/icons/hicolor/scalable/apps
)

# Print configuration summary
message(STATUS "PhotoSync Camera Configuration:")
message(STATUS "  Qt version: ${Qt5_VERSION}")
message(STATUS "  libcamera: ${LIBCAMERA_VERSION}")
message(STATUS "  Build type: ${CMAKE_BUILD_TYPE}")
message(STATUS "  Install prefix: ${CMAKE_INSTALL_PREFIX}")
