#pragma once
#include <QScrollArea>
#include <QWheelEvent>
#include <QScrollBar>
#include <QCursor>

// ─── UI/SettingsPage.hpp ──────────────────────────────────────────────────────
//
// Settings page — runtime control over network backends, crypto selection,
// and display preferences.
//
// Sections:
//   Network Backend — toggle Gossip vs Jami discovery
//   Crypto Backend  — toggle Qt placeholder vs Rust AES-256-GCM
//   Ports           — configure WebSocket and gossip UDP ports
//   About           — version, build flags, commit hash
//
// Design: all settings are persisted via QSettings (INI format in the app
// data directory).  Changes take effect on next app launch; live switching
// of backends is intentionally not supported to avoid half-initialised state.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QLineEdit>
#include <QComboBox>
#include <QSettings>
#include <QList>     // general-purpose; used across settings sections
#include <memory>  // V32.32: unique_ptr for face library fallback
#include "face/FaceLibrary.hpp"  // V32.32: full type needed for unique_ptr dtor
#include "ui/SubTabbed.hpp"      // V32.34+: two-level nav interface

// Forward declarations — avoids pulling in heavy Qt headers into every TU
// that includes SettingsPage.hpp.
QT_BEGIN_NAMESPACE
class QVBoxLayout;
class QComboBox;
class QSpinBox;
class QLabel;
class QPushButton;
class QCheckBox;
class QStackedWidget;
QT_END_NAMESPACE

namespace SnapClone::Core { class AppController; }
namespace SnapClone::Network { class MicroDhtClient; }

namespace SnapClone::UI {

class SegmentedTabStrip;  // V32.34+: shared in-page tab strip

class SettingsPage : public QWidget, public SubTabbed
{
    Q_OBJECT

public:
    explicit SettingsPage(QWidget *parent = nullptr);

    // ── SubTabbed (V32.34+ two-level nav) ────────────────────────────────
    // Settings sub-tabs: General/Camera/Network/Storage/About over the
    // QStackedWidget. setCurrentSubTab routes through selectTab (the same
    // path the in-page strip drives). Null-guarded for pre-buildUi calls.
    [[nodiscard]] int subTabCount() const override;
    [[nodiscard]] int currentSubTab() const override;
    void setCurrentSubTab(int index) override;
    [[nodiscard]] QString subTabLabel(int index) const override;
    [[nodiscard]] QString subTabIcon(int index) const override;   // V32.49+: pull-pill icon

    /// Show the node's stable DHT ID in the settings page.
    void setNodeId(const QString &id);

    /// V32.29: optional AppController pointer for features that need
    /// storage / live state (e.g. ambient-inbox theme derivation needs
    /// to enumerate inbox snap paths). Non-owning. Safe to omit on test
    /// instances — features that need it just no-op.
    ///
    /// V32.32: also triggers a refresh of the face library section so
    /// it can swap from the local fallback Library to the AppController-
    /// owned process-wide instance.
    void setController(SnapClone::Core::AppController *c) {
        m_controller = c;
        refreshFaceLibrarySection();
    }

    bool eventFilter(QObject *obj, QEvent *ev) override;

signals:
    void themeChanged(const QString &paletteId);
    void factoryResetRequested();
    void clearAddressCacheRequested();
    void componentTestRequested();
    void restartRequired(QString reason);

    /// V32.30: user picked "Ambient: Camera" (start=true) or switched away
    /// from it (start=false). MainWindow owns the AmbientCameraDriver and
    /// CameraWidget references — it does the actual subscribe/unsubscribe.
    void ambientCameraRequested(bool start);

    /// V32.30 (Wave 2): same pattern for the inbox-driven mode. start=true
    /// when the user picks "Ambient: Inbox"; start=false when they switch
    /// to any non-inbox carousel entry. MainWindow owns the
    /// AmbientInboxDriver so the auto-refresh on snapReceived survives
    /// SettingsPage destruction/re-entry.
    void ambientInboxRequested(bool start);

    /// V32.33: master kill-switch for ALL ambient sources (inbox + camera).
    /// When user toggles "Disable ambient features" OFF (enabled=false),
    /// MainWindow stops both drivers regardless of which palette is
    /// selected. When toggled ON (enabled=true), drivers can resume per
    /// the current palette choice. Persisted to QSettings under
    /// "ui/ambient_enabled" so the choice survives restart.
    ///
    /// Rationale: ambient drivers run continuous derivation work that
    /// drowns out other UX perf signal. Until measurement establishes
    /// the real perf cost, a global off switch is needed so users (and
    /// developers) can compare with-ambient vs without-ambient app
    /// performance. See bugs/gotchas in TODO.md "Ambient layer cannot
    /// be disabled" for context.
    void ambientMasterEnabledChanged(bool enabled);

    /// V32.32: user tapped the "Test Modal Layers…" button in the
    /// diagnostics section. MainWindow opens ModalLayerTestPanel.
    /// Phase 1 of the modal-layer refactor — see ModalSurface.hpp.
    void modalSurfaceTestRequested();

private slots:
    void loadSettings();
    void saveSettings();
    void onSaveClicked();
    void markDirty();
    void createStickyBar();
    void onResetClicked();
    void onFactoryResetClicked();
    void onTestClicked();
    void onBackendChanged();

protected:
    void resizeEvent(QResizeEvent *e) override;
    void wheelEvent(QWheelEvent *e) override;
private:
    QScrollArea *m_outerScroll = nullptr;
    QLineEdit   *m_displayName = nullptr;
    QWidget     *m_stickyBar   = nullptr;
    bool         m_dirty       = false;
    QScrollArea *m_themeScroll = nullptr; // theme carousel, for wheel forwarding

    // V32.34+: tabbed settings. Instead of one giant scroll with ~16
    // stacked sections, the page is split into top tabs (General / Camera /
    // Network / Storage / About), each a scroll area in a QStackedWidget.
    // The section builders are unchanged — they still take a QVBoxLayout*
    // and add their card to it; buildUi just routes them into the right
    // tab's layout instead of one shared root. The action bar (Save/Reset/
    // Test/Factory) stays pinned below the stack, shared across all tabs.
    QStackedWidget *m_tabStack = nullptr;
    SegmentedTabStrip *m_settingsTabs = nullptr;  // V32.34+: shared tab strip
    void selectTab(int index);
    /// Build a tab page: a CarouselAwareScrollArea wrapping a container with
    /// a QVBoxLayout. Returns the inner layout so callers add sections to it,
    /// and the scroll area is added to m_tabStack. `outIsThemeHost` lets the
    /// caller know which tab hosts the appearance carousel (for wheel
    /// forwarding wiring), since only one tab has it.
    QVBoxLayout *makeTabPage(QWidget *parentForWidth);

    void buildUi();
    // QVBoxLayout* is fully declared via the QT_BEGIN_NAMESPACE block above —
    // no 'class' keyword needed here.
    void buildIdentitySection(QVBoxLayout *root, QWidget *container);
    void buildAppearanceSection(QVBoxLayout *root);
    void buildDiagnosticsSection(QVBoxLayout *root);
    void buildDebugFlagsSection(QVBoxLayout *root);
    void buildPrivacySection(QVBoxLayout *root);
    void buildSoundsSection(QVBoxLayout *root);       // ringtone gain/mute + mic kill-switch
    void buildVoiceTuningSection(QVBoxLayout *root);  // V32.49+: live mic/voice tuning knobs
    void buildFaceLibrarySection(QVBoxLayout *root);  // V32.32: Phase 1 stats
    void buildCameraSection(QVBoxLayout *root);       // V32.34+: policy + tester UI
    void buildAccessibilitySection(QVBoxLayout *root);// V32.34+: speech boost on playback
    void buildStorageSection(QVBoxLayout *root);      // V32.33: disk usage + clear
    void buildExportSection(QVBoxLayout *root);       // V32.34+: monthly export + auto-sync
    void buildSecuritySection(QVBoxLayout *root);     // V32.33: TOFU pending key changes (S-20)
    void buildAmbientSection(QVBoxLayout *root);      // V32.33: master kill switch
    void buildNetworkSection(QVBoxLayout *root);
    void buildStunSection   (QVBoxLayout *root);
    void buildCryptoSection (QVBoxLayout *root);
    void buildPortsSection  (QVBoxLayout *root);
    void buildAboutSection  (QVBoxLayout *root);
    void buildBetaTestersSection(QVBoxLayout *root);  // V32.34+: micro-DHT test peer toggle

    // ── Network ──
    QComboBox   *m_discoveryBackend  = nullptr;
    QCheckBox   *m_upnpEnabled       = nullptr;
    QCheckBox   *m_jamiEnabled       = nullptr;

    // ── STUN servers ──
    QCheckBox   *m_stunSfl           = nullptr; // stun.jami.net (Savoir-faire Linux)
    QCheckBox   *m_stunNextcloud     = nullptr; // stun.nextcloud.com
    QCheckBox   *m_stunLinphone      = nullptr; // stun.linphone.org (Belledonne / FOSS VoIP)
    QCheckBox   *m_stunEkiga         = nullptr; // stun.ekiga.net (GNOME)
    QCheckBox   *m_stunFramasoft     = nullptr; // stun.framasoft.org (French FOSS)
    QCheckBox   *m_stunGoogle        = nullptr; // stun.l.google.com (convenience)
    QCheckBox   *m_stunCloudflare    = nullptr; // stun.cloudflare.com
    QComboBox   *m_relayPreset       = nullptr; // known relay presets
    QLineEdit   *m_relayUrl          = nullptr; // custom ws:// relay URL
    QLabel      *m_relayPresetNote   = nullptr; // description of selected preset

    // ── Crypto ──
    QComboBox   *m_cryptoBackend     = nullptr;

    // ── Beta testers (V32.34+): local micro-DHT test peer ──
    // When enabled, DHT publish/lookup can be exercised against a local
    // `sc-microdht` node instead of the real Jami DHT. These widgets drive the
    // beta/micro_dht_* settings keys; the actual routing lives in the DHT
    // abstraction. m_microDht is created lazily for the ping/status check.
    QCheckBox   *m_betaMicroDhtEnabled = nullptr;
    QLineEdit   *m_betaMicroDhtHost    = nullptr;  // host:port, default 127.0.0.1:4544
    QLabel      *m_betaMicroDhtStatus  = nullptr;  // ping/status indicator
    Network::MicroDhtClient *m_microDht = nullptr;

    // ── Debug flags ──
    // Mirror of the live process-wide DebugFlags singleton. Toggling
    // these applies IMMEDIATELY to the runtime (so A/B testing is one
    // click) AND marks the page dirty so Save persists across restarts.
    // See include/debug/DebugFlags.hpp for what each flag actually
    // disables and why.
    QCheckBox   *m_dbgNoFsmTuning         = nullptr;
    QCheckBox   *m_dbgNoMonitoringBlocking = nullptr;
    QCheckBox   *m_dbgVerboseFsm          = nullptr;
    QCheckBox   *m_dbgMobileDisplay       = nullptr;  // V32.34+
    QLineEdit   *m_autoSyncPathEdit       = nullptr;  // V32.34+ export section

    // ── Ports ──
    QSpinBox    *m_wsPort            = nullptr;
    QSpinBox    *m_gossipPort        = nullptr;

    // ── Identity ──
    QLabel      *m_nodeIdLabel       = nullptr;
    QLabel      *m_qrLabel           = nullptr;
    QPushButton *m_copyBtn           = nullptr;

    // ── About ──
    QLabel      *m_buildFlagsLabel   = nullptr;

    QPushButton *m_saveBtn           = nullptr;
    QPushButton *m_resetBtn          = nullptr;
    QPushButton *m_factoryResetBtn   = nullptr;
    QPushButton *m_testBtn           = nullptr;

    bool m_restartPending = false;

    /// V32.29: non-owning. Provides storage access for the ambient-inbox
    /// theme derivation. Set by MainWindow via setController() after
    /// construction. Nullptr-safe — features that need it just no-op.
    SnapClone::Core::AppController *m_controller = nullptr;

    // V32.32: Face library section widgets. Built lazily by
    // buildFaceLibrarySection; refreshed by refreshFaceLibrarySection()
    // when setController() arrives (the AppController-owned library
    // replaces the local fallback). All may stay nullptr when the
    // section concludes it can't usefully run (SNAPCLONE_RUST_FACE off).
    QLabel      *m_faceStatsLabel   = nullptr;
    QPushButton *m_faceWipeBtn      = nullptr;
    QCheckBox   *m_faceEnabledToggle = nullptr;
    // Fallback Library used by the face section when no AppController
    // is attached. Lazy-constructed on first need; replaced (logically;
    // we don't destroy it) when setController() arrives — at that point
    // currentFaceLibrary() returns the controller's instance instead.
    std::unique_ptr<SnapClone::Face::Library> m_faceLibFallback;

    void refreshFaceLibrarySection();

    // V32.33: Storage section widgets. Built by buildStorageSection;
    // refreshed by refreshStorageSection() on section entry and after
    // the clear button runs. The total is computed lazily by walking
    // the app's data + cache directories; that walk happens on a worker
    // thread (recordings dirs can hold hundreds of MB), with the result
    // dispatched back to the GUI thread via QPointer-guarded callback.
    QLabel      *m_storageTotalLabel  = nullptr;
    QPushButton *m_storageClearBtn    = nullptr;
    void refreshStorageSection();

    // V32.33: Security section state (CRYPTO_REVIEW.md S-20).
    // Holds the container into which pending-key-change cards are
    // injected on every refreshSecuritySection() call. We rebuild
    // the cards rather than diff them — the list is tiny and changes
    // rarely (typically 0 entries).
    QWidget *m_securityListContainer = nullptr;
    QLabel  *m_securityEmptyLabel    = nullptr;
    void refreshSecuritySection();
    /// Returns the active Face::Library — the AppController-owned one
    /// when set via setController(), otherwise a local fallback created
    /// on first call. Never null.
    SnapClone::Face::Library *currentFaceLibrary();
};

// ─── Settings keys ────────────────────────────────────────────────────────────
// Centralised string constants prevent typos when reading/writing QSettings.
namespace SettingsKey {
    inline constexpr const char *kDiscoveryBackend = "network/discoveryBackend";
    inline constexpr const char *kJamiEnabled      = "network/jamiEnabled";
    inline constexpr const char *kUpnpEnabled      = "network/upnpEnabled";
    inline constexpr const char *kCryptoBackend    = "crypto/backend";
    inline constexpr const char *kWsPort           = "network/wsPort";
    inline constexpr const char *kGossipPort       = "network/gossipPort";
    // STUN server enable flags (bitmask stored as int)
    inline constexpr const char *kStunServersEnabled = "network/stunServers";
    // Default: all FOSS servers on, Google off
    // Bit 0: SFL/Jami, Bit 1: Nextcloud, Bit 2: Linphone, Bit 3: Ekiga
    // Bit 4: Framasoft, Bit 5: Google, Bit 6: Cloudflare
    inline constexpr int kStunDefaultMask = 0b0011111; // all FOSS on, Google+CF off
} // namespace SettingsKey

} // namespace SnapClone::UI
