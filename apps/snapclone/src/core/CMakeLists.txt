add_library(snapclone_core STATIC
    AppController.cpp
    PowerManager.cpp
    ActivityGate.cpp
    SleepState.cpp
    SingleInstanceGuard.cpp
    AppController_connect.cpp
    AppController_commands.cpp
    AppContext.cpp
    LocationManager.cpp
    AppController_misc.cpp
    SessionManager.cpp
    SnapRouter.cpp
    VoiceWirePacket.cpp              # group voice: on-wire envelope (pure framing)
    MediaMetadataStripper.cpp        # privacy: strip EXIF/GPS before send (mediashare FFI)
    GroupMediaSender.cpp             # group media: strip→seal→chunk→keyshare (mediashare FFI)
    Identity.cpp
    AccountManager.cpp
    VCardIO.cpp
    TieredDeliveryPolicy.cpp
    LandmarkHash.cpp                # V32.31: privacy-preserving image fingerprints
    VoiceNoteRecorder.cpp           # V32.46+: ephemeral voice notes (record→Opus blob; inert w/o codec)
    VoiceNotePlayer.cpp             # V32.46+: sequential voice-note playback (one-at-a-time queue)
    # Headers must be listed so AUTOMOC can find Q_OBJECT macros in include/
    ${PROJECT_SOURCE_DIR}/include/core/AppController.hpp
    ${PROJECT_SOURCE_DIR}/include/core/AppContext.hpp
    ${PROJECT_SOURCE_DIR}/include/core/PowerManager.hpp
    ${PROJECT_SOURCE_DIR}/include/core/ActivityGate.hpp   # Q_OBJECT (interactiveChanged)
    ${PROJECT_SOURCE_DIR}/include/core/SleepState.hpp   # V32.34+: Q_OBJECT (sleepStateChanged)
    ${PROJECT_SOURCE_DIR}/include/core/SingleInstanceGuard.hpp  # V32.34+: Q_OBJECT (raiseRequested)
    ${PROJECT_SOURCE_DIR}/include/core/SessionManager.hpp
    ${PROJECT_SOURCE_DIR}/include/core/SnapRouter.hpp
    ${PROJECT_SOURCE_DIR}/include/core/GroupMediaSender.hpp  # group media: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/core/VoiceNoteRecorder.hpp  # V32.46+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/core/VoiceNotePlayer.hpp    # V32.46+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/core/LocationManager.hpp
)

target_include_directories(snapclone_core
    PUBLIC
        ${PROJECT_SOURCE_DIR}/include
)

target_link_libraries(snapclone_core
    PUBLIC
        snapclone_warnings
        snapclone_network
        snapclone_storage
        snapclone_crypto
        snapclone_face   # V32.32: process-wide Face::Library owned by AppController
        Qt6::Core
        Qt6::Gui      # V32.21: QGuiApplication::applicationStateChanged in AppContext
        Qt6::Network  # V32.21: QNetworkInformation in AppContext
        Qt6::Positioning
)

# ─── Group media (dedup + keyshare + EXIF/GPS strip) ─────────────────────────
# GroupMediaSender / MediaMetadataStripper call the snapclone-mediashare FFI
# (sc_chunk_store_*, sc_media_*, sc_image_strip_metadata). Both guard the FFI
# behind SNAPCLONE_RUST_MEDIASHARE internally (stub out when off) so they compile
# unconditionally — but when the crate IS built the executable must link it, or
# ld fails with undefined references. The imported lib is IMPORTED GLOBAL (root
# CMakeLists); its symbols resolve at the final exe link.
if(SNAPCLONE_RUST_MEDIASHARE)
    target_link_libraries(snapclone_core PUBLIC snapclone_rust_mediashare_lib)
endif()

set_target_properties(snapclone_core PROPERTIES AUTOMOC ON)

# ─── Group-radio push-to-talk session ────────────────────────────────────────
# GroupRadioSession orchestrates the Rust floor controller (snapclone-group) and
# transmit gate (snapclone-audio) for group half-duplex voice. Only compiled when
# both crates are available, so the binary never carries undefined FFI symbols in
# a build with either feature off. The header is listed for AUTOMOC (Q_OBJECT).
# Both imported lib targets are IMPORTED GLOBAL (defined in the root CMakeLists),
# so they're referenceable here. Their symbols resolve at the final exe link.
if(SNAPCLONE_RUST_GROUP AND SNAPCLONE_RUST_AUDIO)
    target_sources(snapclone_core
        PRIVATE
            GroupRadioSession.cpp
            GroupVoiceCapture.cpp
            ${PROJECT_SOURCE_DIR}/include/core/GroupRadioSession.hpp  # Q_OBJECT
            ${PROJECT_SOURCE_DIR}/include/core/GroupVoiceCapture.hpp  # Q_OBJECT
    )
    target_link_libraries(snapclone_core
        PUBLIC
            snapclone_rust_group_lib
            snapclone_rust_audio_lib
            Qt6::Multimedia   # QAudioSource mic capture in GroupVoiceCapture
    )

    # The Opus sender/receiver reference the codec symbols (sc_opus_*), which the
    # audio crate exports only when built with the `opus` feature. Compile them
    # only when that feature is on, so a no-libopus build still links.
    if(SNAPCLONE_OPUS_CODEC)
        target_sources(snapclone_core
            PRIVATE
                GroupVoiceSender.cpp
                GroupVoiceReceiver.cpp
                ${PROJECT_SOURCE_DIR}/include/core/GroupVoiceSender.hpp   # Q_OBJECT
                ${PROJECT_SOURCE_DIR}/include/core/GroupVoiceReceiver.hpp # Q_OBJECT
        )
    endif()
endif()
