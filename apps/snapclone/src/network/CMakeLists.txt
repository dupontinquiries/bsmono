add_library(snapclone_network STATIC
    NetworkManager.cpp
    NatTraversal.cpp
    GossipDiscovery.cpp
    WebSocketTransport.cpp
    NetworkDiagnostics.cpp
    FirewallPortManager.cpp
    DataBusClient.cpp
    DhtResourceManager.cpp
    UPnPPortMapper.cpp
    RelayTransport.cpp
    MagicWormholeRelayTransport.cpp
    DirectDiscovery.cpp
    UdpChannel.cpp
    JamiAdapter.cpp
    JamiAdapter_dht_impl.cpp
    PollinatorDiscovery.cpp
    NetworkStats.cpp
    MicroDhtClient.cpp
    DhtService.cpp
    # Headers — listed so AUTOMOC finds Q_OBJECT macros in include/
    ${PROJECT_SOURCE_DIR}/include/network/NetworkManager.hpp
    ${PROJECT_SOURCE_DIR}/include/network/GossipDiscovery.hpp
    ${PROJECT_SOURCE_DIR}/include/network/WebSocketTransport.hpp
    ${PROJECT_SOURCE_DIR}/include/network/NetworkDiagnostics.hpp
    ${PROJECT_SOURCE_DIR}/include/network/FirewallPortManager.hpp
    ${PROJECT_SOURCE_DIR}/include/network/UPnPPortMapper.hpp
    ${PROJECT_SOURCE_DIR}/include/network/RelayTransport.hpp
    ${PROJECT_SOURCE_DIR}/include/network/MagicWormholeRelayTransport.hpp
    ${PROJECT_SOURCE_DIR}/include/network/DirectDiscovery.hpp
    ${PROJECT_SOURCE_DIR}/include/network/UdpChannel.hpp
    ${PROJECT_SOURCE_DIR}/include/network/JamiAdapter.hpp
    ${PROJECT_SOURCE_DIR}/include/network/NatTraversal.hpp
    ${PROJECT_SOURCE_DIR}/include/network/PollinatorDiscovery.hpp
    ${PROJECT_SOURCE_DIR}/include/network/DataBusClient.hpp
    ${PROJECT_SOURCE_DIR}/include/network/DhtResourceManager.hpp
    ${PROJECT_SOURCE_DIR}/include/network/NetworkStats.hpp
    ${PROJECT_SOURCE_DIR}/include/network/MicroDhtClient.hpp
    ${PROJECT_SOURCE_DIR}/include/network/DhtService.hpp
)

target_include_directories(snapclone_network
    PUBLIC
        ${PROJECT_SOURCE_DIR}/include
)

target_link_libraries(snapclone_network
    PUBLIC
        snapclone_warnings
        snapclone_debug
        Qt6::Core
        Qt6::Network
        Qt6::WebSockets
        Qt6::DBus
)

# Link the appropriate backend library.
if(SNAPCLONE_JAMI_BACKEND)
    # PkgConfig::jami is the imported target created by pkg_check_modules(IMPORTED_TARGET).
    # It carries the correct include directories, compile flags, and link libraries
    # from the jami.pc file automatically — no manual JAMI_INCLUDE_DIRS needed.
    target_link_libraries(snapclone_network PUBLIC PkgConfig::jami)
elseif(SNAPCLONE_OPENDHT_BACKEND)
    # opendht_with_c provides headers (include/ + c/) and links the C++ opendht lib.
    target_link_libraries(snapclone_network PUBLIC opendht_with_c)
    # Compile the C API wrapper (c/opendht.cpp) directly into snapclone_network.
    # We set OPENDHT_C=OFF to skip opendht's own build of this file (it would
    # create a separate shared library we don't need). Instead we compile it
    # as a plain C++ source in our own library so the C wrapper symbols are
    # available at link time without an extra .so dependency.
    target_sources(snapclone_network PRIVATE
        "${OPENDHT_SUBMODULE_DIR}/c/opendht.cpp")
endif()

set_target_properties(snapclone_network PROPERTIES AUTOMOC ON)
