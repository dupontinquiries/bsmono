find_package(Qt6 REQUIRED COMPONENTS Test)

# ─── Helper macro to register a Qt Test executable ───────────────────────────
macro(snapclone_add_test name src)
    qt_add_executable(${name} ${src})
    set_target_properties(${name} PROPERTIES AUTOMOC ON)
    target_include_directories(${name} PRIVATE ${PROJECT_SOURCE_DIR}/include)
    target_link_libraries(${name}
        PRIVATE
            snapclone_warnings
            snapclone_core
            snapclone_network
            snapclone_storage
            snapclone_crypto
            Qt6::Core
            Qt6::Test
    )
    add_test(NAME ${name} COMMAND ${name})
endmacro()

snapclone_add_test(test_StorageManager  test_StorageManager.cpp)
snapclone_add_test(test_CryptoService   test_CryptoService.cpp)
snapclone_add_test(test_GossipDiscovery test_GossipDiscovery.cpp)
snapclone_add_test(test_AddressClassification test_AddressClassification.cpp)

# ─── Pure-logic harness (no Qt) ──────────────────────────────────────────────
# PressFsm is the Qt-free decision core behind PressGesture; its simulation
# harness replays synthetic novice-style input and needs neither Qt nor the
# snapclone libraries, so it builds as a plain C++ executable.
# Trace-channel policy: pure C++, no Qt link needed (that is the point of
# keeping TraceChannels.cpp Qt-free — the debug policy stays unit-testable).
add_executable(test_TraceChannels
    test_TraceChannels.cpp
    ${PROJECT_SOURCE_DIR}/src/debug/TraceChannels.cpp)
target_include_directories(test_TraceChannels PRIVATE ${PROJECT_SOURCE_DIR}/include)
add_test(NAME test_TraceChannels COMMAND test_TraceChannels)

add_executable(test_PressFsm test_PressFsm.cpp)
target_include_directories(test_PressFsm PRIVATE ${PROJECT_SOURCE_DIR}/include)
target_link_libraries(test_PressFsm PRIVATE snapclone_warnings)
add_test(NAME test_PressFsm COMMAND test_PressFsm)

# EnergyPolicy: pure decision core behind ActivityGate's timer gating. No Qt.
add_executable(test_EnergyPolicy test_EnergyPolicy.cpp)
target_include_directories(test_EnergyPolicy PRIVATE ${PROJECT_SOURCE_DIR}/include)
target_link_libraries(test_EnergyPolicy PRIVATE snapclone_warnings)
add_test(NAME test_EnergyPolicy COMMAND test_EnergyPolicy)

# SwipeZone + MotionSmooth: pure page-vs-submenu gesture rule and the
# frame-rate-independent page glide. No Qt.
add_executable(test_SwipeMotion test_SwipeMotion.cpp)
target_include_directories(test_SwipeMotion PRIVATE ${PROJECT_SOURCE_DIR}/include)
target_link_libraries(test_SwipeMotion PRIVATE snapclone_warnings)
add_test(NAME test_SwipeMotion COMMAND test_SwipeMotion)

# SubPageDetent: pure resistance + progress curve for the vertical sub-page
# (sub-tab) gesture — buffer/detent, stickier-toward-commit, grow-text. No Qt.
add_executable(test_SubPageDetent test_SubPageDetent.cpp)
target_include_directories(test_SubPageDetent PRIVATE ${PROJECT_SOURCE_DIR}/include)
target_link_libraries(test_SubPageDetent PRIVATE snapclone_warnings)
add_test(NAME test_SubPageDetent COMMAND test_SubPageDetent)

# RadioTransmitLogic: pure orchestration between the (Rust) transmit gate and
# floor control — transmit/assert/release decisions + keepalive cadence for
# group push-to-talk. No Qt, no FFI.
add_executable(test_RadioTransmitLogic test_RadioTransmitLogic.cpp)
target_include_directories(test_RadioTransmitLogic PRIVATE ${PROJECT_SOURCE_DIR}/include)
target_link_libraries(test_RadioTransmitLogic PRIVATE snapclone_warnings)
add_test(NAME test_RadioTransmitLogic COMMAND test_RadioTransmitLogic)
