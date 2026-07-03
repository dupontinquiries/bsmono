# ─── snapclone_ui ─────────────────────────────────────────────────────────────
# All Qt widget sources.  Headers are listed explicitly so AUTOMOC finds
# Q_OBJECT macros that live in include/ (not alongside the .cpp files).
# ─────────────────────────────────────────────────────────────────────────────

add_library(snapclone_ui STATIC
    MainWindow.cpp
    LoginWidget.cpp
    InboxWidget.cpp
    SnapFeedWidget.cpp
    CameraWidget.cpp
    camera/QtCameraBackend.cpp
    camera/GstLaunchBackend.cpp
    camera/VcmController.cpp        # V32.32: dw9719 focus motor control
    camera/FocusSweep.cpp           # V32.32: CDAF sharpness + sweep
    camera/FocusReticle.cpp         # V32.34+: tap-to-focus visual feedback
    camera/GlassScrim.cpp           # V32.38+: frosted-glass full-bleed scrim
    camera/GlassIconButton.cpp      # V32.38e: mottled glass icon button + ripple
    camera/CameraRegistry.cpp       # V32.32: typed camera metadata table
    camera/CameraBridgeManager.cpp
    camera/CameraPolicy.cpp         # V32.34+: timeouts/limits config
    camera/CameraTester.cpp         # V32.34+: dynamic node probing
    camera/CameraHealthService.cpp  # V32.34+: cached health probes
    camera/FramePixelOps.cpp        # V32.39+: shared SIMD pixel ops (swap, YUV→RGB)
    video/VideoRecorder.cpp
    video/VideoTranscoder.cpp
    video/VideoEncoderBackend.cpp
    video/VideoSnapViewerDialog.cpp
    video/ShutterButton.cpp
    video/RecordingMetadata.cpp
    wayfinder/WayfinderPage.cpp
    posture/PostureEngine.cpp           # V32.42+: proximity screensaver state machine
    posture/DimOverlay.cpp              # V32.42+: in-app dim overlay
    posture/VoiceActivityTracker.cpp    # V32.42+: 5-min voice recency (arming)
    posture/IioSensorProxySource.cpp    # V32.42+: iio-sensor-proxy proximity + accel
    posture/PostureController.cpp       # V32.42+: window-level glue
    FriendListWidget.cpp
    DiagnosticsDialog.cpp
    SwipeNavigator.cpp
    TabBar.cpp
    SegmentedTabStrip.cpp           # V32.34+: reusable in-page tab strip
    SmartRibbon.cpp                 # V32.34+: bottom tabs+actions ribbon (prototype)
    UnifiedNavBar.cpp               # V32.34+: unified bottom bar (two-level nav view)
    ContactCard.cpp                 # V32.34+: expandable contact card w/ swipe-out submenu
    SnapExporter.cpp                # V32.34+: zip/tar export of snap media
    ExportReminderCard.cpp          # V32.34+: monthly export reminder card
    MobileDialog.cpp
    TouchFeedback.cpp
    ChevronConfirm.cpp
    UxGuide.cpp
    QrScanDialog.cpp
    SvgIcon.cpp
    SettingsPage.cpp
    ComponentTestWindow.cpp
    SnapViewerDialog.cpp
    HelpManualDialog.cpp            # V32.43+: in-app user manual popup
    SnapComposerDialog.cpp
    ContactEditorDialog.cpp
    ContactsPage.cpp
    ContactPickerDialog.cpp
    ConversationDetailDialog.cpp
    IncomingCallDialog.cpp           # group voice: incoming-call ring
    RingtoneSynth.cpp                # ringtone synthesis (shared w/ tools/ringtone_workshop)
    PagePeelHint.cpp
    QrCodeRenderer.cpp
    ScrollAreas.cpp
    AmbientPaletteDeriver.cpp        # V32.29: dynamic theme derivation
    AmbientBlobs.cpp                 # V32.30: pulsing color overlay
    AmbientCameraDriver.cpp          # V32.30: live-viewfinder theme driver
    VirtualKeyboardWatcher.cpp       # V32.34+: on-screen keyboard avoidance
    UiDebugFlags.cpp                 # V32.34+: settings-toggled UI log gates
    MonthlyExportReminder.cpp        # V32.34+: first-of-month export prompt
    AmbientInboxDriver.cpp           # V32.30 wave 2: snap-arrival theme driver
    PressGesture.cpp                 # V32.30: tap/long-press/drag-cancel FSM
    AddLandmarkDialog.cpp            # V32.31: landmark creation UI
    ModalSurface.cpp                 # V32.32: unified overlay/dialog/toast layer mgr
    ModalLayerTestPanel.cpp          # V32.32: ModalSurface test harness
    ThumbnailService.cpp             # V32.34+: async file→pixmap loader
    GroupVoiceBar.cpp                # group voice: on-air control surface
    GroupVoiceRoster.cpp             # group voice: channel roster + active speaker
    ConversationSecurityBar.cpp      # security tier banner (standard/securing/private)
    MediaSendSummary.cpp             # group media: strip/dedup/encrypt send receipt
    GroupVoiceCoordinator.cpp        # group voice: wires backend ↔ the voice widgets
    VoiceNetworkBridge.cpp           # group voice: coordinator seams ↔ network pipe
    # Headers — listed so AUTOMOC finds Q_OBJECT macros in include/ui/
    ${PROJECT_SOURCE_DIR}/include/ui/MainWindow.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/LoginWidget.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/InboxWidget.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/SnapFeedWidget.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/CameraWidget.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/camera/CameraBackend.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/camera/QtCameraBackend.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/camera/GstLaunchBackend.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/camera/CameraBridgeManager.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/camera/CameraPolicy.hpp    # V32.34+: Q_OBJECT (signal changed)
    ${PROJECT_SOURCE_DIR}/include/ui/camera/CameraTester.hpp    # V32.34+: Q_OBJECT (signals)
    ${PROJECT_SOURCE_DIR}/include/ui/camera/CameraHealthService.hpp  # V32.34+: Q_OBJECT (signals)
    ${PROJECT_SOURCE_DIR}/include/ui/camera/FramePixelOps.hpp  # V32.39+: shared SIMD pixel ops
    ${PROJECT_SOURCE_DIR}/include/ui/camera/FocusReticle.hpp    # V32.34+: Q_OBJECT (Q_PROPERTY)
    ${PROJECT_SOURCE_DIR}/include/ui/camera/GlassScrim.hpp      # V32.38+: Q_OBJECT (frosted scrim)
    ${PROJECT_SOURCE_DIR}/include/ui/camera/GlassIconButton.hpp # V32.38e: Q_OBJECT (Q_PROPERTY anim)
    ${PROJECT_SOURCE_DIR}/include/ui/video/VideoRecorder.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/video/VideoTranscoder.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/video/VideoSnapViewerDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/video/ShutterButton.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/video/RecordingMetadata.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/wayfinder/WayfinderPage.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/posture/PostureSource.hpp          # V32.42+: Q_OBJECT (iface + fake)
    ${PROJECT_SOURCE_DIR}/include/ui/posture/PostureEngine.hpp          # V32.42+: Q_OBJECT (state machine)
    ${PROJECT_SOURCE_DIR}/include/ui/posture/DimOverlay.hpp             # V32.42+: Q_OBJECT (Q_PROPERTY)
    ${PROJECT_SOURCE_DIR}/include/ui/posture/VoiceActivityTracker.hpp   # V32.42+: Q_OBJECT (signal)
    ${PROJECT_SOURCE_DIR}/include/ui/posture/IioSensorProxySource.hpp   # V32.42+: Q_OBJECT (D-Bus slots)
    ${PROJECT_SOURCE_DIR}/include/ui/posture/PostureController.hpp      # V32.42+: Q_OBJECT (glue)
    ${PROJECT_SOURCE_DIR}/include/ui/FriendListWidget.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/GroupVoiceBar.hpp       # group voice: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/GroupVoiceRoster.hpp    # group voice: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/ConversationSecurityBar.hpp  # security tier: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/MediaSendSummary.hpp    # group media: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/GroupVoiceCoordinator.hpp  # group voice coordinator: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/VoiceNetworkBridge.hpp     # group voice transport bridge: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/DiagnosticsDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/SwipeNavigator.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/TabBar.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/SegmentedTabStrip.hpp   # V32.34+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/SmartRibbon.hpp         # V32.34+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/ExportReminderCard.hpp  # V32.34+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/UnifiedNavBar.hpp        # V32.34+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/ContactCard.hpp          # V32.34+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/MobileDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/TouchFeedback.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/ChevronConfirm.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/UxGuide.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/QrScanDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/SvgIcon.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/SettingsPage.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/ComponentTestWindow.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/SnapViewerDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/HelpManualDialog.hpp   # V32.43+: Q_OBJECT (manual popup)
    ${PROJECT_SOURCE_DIR}/include/ui/SnapComposerDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/ContactEditorDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/ContactsPage.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/ContactPickerDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/ConversationDetailDialog.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/IncomingCallDialog.hpp  # incoming-call ring: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/PagePeelHint.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/QrCodeRenderer.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/ScrollAreas.hpp
    ${PROJECT_SOURCE_DIR}/include/ui/AmbientBlobs.hpp   # V32.30: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/AmbientCameraDriver.hpp # V32.30: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/VirtualKeyboardWatcher.hpp # V32.34+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/UiDebugFlags.hpp # V32.34+: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/AmbientInboxDriver.hpp  # V32.30 wave 2: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/PressGesture.hpp   # V32.30: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/AddLandmarkDialog.hpp # V32.31: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/ModalSurface.hpp    # V32.32: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/ModalLayerTestPanel.hpp # V32.32: Q_OBJECT
    ${PROJECT_SOURCE_DIR}/include/ui/ThumbnailService.hpp # V32.34+: Q_OBJECT
)

target_include_directories(snapclone_ui
    PUBLIC
        ${PROJECT_SOURCE_DIR}/include
)

target_link_libraries(snapclone_ui
    PUBLIC
        snapclone_warnings
        snapclone_core
        snapclone_network
        snapclone_face   # V32.32: Phase 1 face library settings UI
        Qt6::Core
        Qt6::Gui
        Qt6::Widgets
    Qt6::Svg
        Qt6::Multimedia
        Qt6::MultimediaWidgets
        Qt6::Network
        Qt6::Concurrent   # V32.32: async CDAF sweep + VCM open off main thread
        Qt6::DBus         # V32.42+: iio-sensor-proxy (proximity) for posture engine
)

# ─── libcamera backend (guarded) ─────────────────────────────────────────────
# When SNAPCLONE_LIBCAMERA was found (set in the root CMakeLists), compile the
# direct-libcamera backend plus the ported converter / buffer-mmap helpers into
# snapclone_ui and link the libcamera import target. LibcameraBackend.hpp is
# listed so AUTOMOC processes its Q_OBJECT.
if(SNAPCLONE_LIBCAMERA)
    target_sources(snapclone_ui PRIVATE
        camera/LibcameraBackend.cpp
        camera/libcam/converter.cpp
        camera/libcam/image.cpp
        ${PROJECT_SOURCE_DIR}/include/ui/camera/LibcameraBackend.hpp
    )
    target_link_libraries(snapclone_ui PRIVATE PkgConfig::LIBCAMERA)
endif()

if(SNAPCLONE_RUST_SALIENCY)
    target_link_libraries(snapclone_ui PRIVATE snapclone_rust_saliency_lib)
endif()

# VQ image codec FFI (snapclone-vqcodec). Linked when the option is ON; the
# archive only contributes objects once a TU actually calls sc_vq_*, so this is
# benign until the snap send/receive path is wired to it.
if(SNAPCLONE_RUST_VQCODEC)
    target_link_libraries(snapclone_ui PRIVATE snapclone_rust_vqcodec_lib)
endif()

# V32.35+ UNIFIED-MAILBOX FFI: InboxWidget calls sc_menu_* (snapclone-menu) and
# sc_reqmb_* (snapclone-mailbox) for the dynamic request submenus. Unlike the
# group lib above, these DO have a live C++ caller now (InboxWidget), so the
# link is enabled. Both .a's are built by the always-on blocks in the root
# CMakeLists. PRIVATE: the symbols are used inside snapclone_ui's TU's, not
# re-exported.
target_link_libraries(snapclone_ui PRIVATE
    snapclone_rust_menu_lib
    snapclone_rust_mailbox_lib
)

# V32.34+ GROUP FFI: the snapclone_rust_group_lib staticlib + its header
# (include/group/snapclone_group_ffi.h) and the top-level SNAPCLONE_RUST_GROUP
# build option are in place and the .a is verified to build. The link line
# below is intentionally NOT enabled yet: it should be turned on in the SAME
# change that adds the first C++ caller (the group session, likely in core or
# network), so we don't add a dead link dependency to a target that references
# no group symbols. When that consumer lands, add to ITS CMakeLists:
#   if(SNAPCLONE_RUST_GROUP)
#       target_link_libraries(<consumer> PRIVATE snapclone_rust_group_lib)
#   endif()

if(SNAPCLONE_RUST_AUDIO)
    target_link_libraries(snapclone_ui PRIVATE snapclone_rust_audio_lib)
endif()

set_target_properties(snapclone_ui PROPERTIES AUTOMOC ON)

# ─── Embedded SVG icons (avoid filesystem path lookup at runtime) ──
# The .qrc file in the project root defines aliases so paths are :/icons/<name>.svg
qt_add_resources(snapclone_ui "icons"
    PREFIX "/icons"
    BASE "${PROJECT_SOURCE_DIR}/assets/icons"
    FILES
        ${PROJECT_SOURCE_DIR}/assets/icons/beacon.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/camera.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/edit.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/export.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/friends.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/import.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/inbox.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/landmark.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/send.svg
        ${PROJECT_SOURCE_DIR}/assets/icons/settings.svg
)
