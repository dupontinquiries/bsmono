#pragma once
// Qt6 network reachability (instant offline/online detection)
#include <optional>
// Forward declare to avoid heavy include in header

// ─── Core/AppController.hpp ──────────────────────────────────────────────────
//
// AppController is the single composition root of the application.
//
// Responsibilities:
//   • Own (via unique_ptr) every top-level subsystem.
//   • Wire signals and slots between subsystems at startup.
//   • Expose a narrow, intention-revealing API to the UI layer.
//
// What it is NOT:
//   • A god object — it delegates all real work to subsystems.
//   • A UI component — it holds no widget pointers.
//
// Threading model:
//   AppController lives on the main thread.  Network work is dispatched to
//   a dedicated QThread managed by NetworkManager.  Storage I/O runs on a
//   QThreadPool via QtConcurrent.  Signals that cross thread boundaries carry
//   only value types (see Types.hpp) so no synchronization is needed at the
//   call sites.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <memory>

#include "core/Types.hpp"

namespace SnapClone::Network { class NetworkManager; }
namespace SnapClone::Network { class IDiscovery;      }
namespace SnapClone::Network { class FirewallPortManager;   }
namespace SnapClone::Network { class PollinatorDiscovery;   }
namespace SnapClone::Storage { class StorageManager; }
namespace SnapClone::Crypto  { class ICryptoService; }
namespace SnapClone::Face    { class Library;       }  // V32.32
namespace SnapClone::Face    { class Detector;      }  // V32.32

namespace SnapClone::Core {

class SessionManager;
class SnapRouter;
class AppContextProvider;
class PowerManager;   // V32.34+ battery: screen/battery/thermal signals
class SleepStateMachine;  // V32.34+ battery: shared sleep-state machine

class AppController : public QObject
{
    Q_OBJECT

public:
    explicit AppController(QObject *parent = nullptr);
    ~AppController() override;

    // Initialise all subsystems and begin peer discovery.
    void initialize();

    // ── Accessors (non-owning pointers for dependency injection into UI) ──
    [[nodiscard]] Network::NetworkManager     *networkManager()   const;
    [[nodiscard]] Network::FirewallPortManager  *firewallManager()      const;
    [[nodiscard]] Network::PollinatorDiscovery  *pollinatorDiscovery()  const;
    [[nodiscard]] Network::IDiscovery          *dhtDiscovery()    const;
    [[nodiscard]] QString                       stableId()        const;
    [[nodiscard]] Storage::StorageManager     *storageManager()  const;

    /// V32.34+ battery: the process-wide PowerManager. Non-null after
    /// initialize() returns. UI components (notably CameraWidget) subscribe
    /// to its screenActiveChanged signal to pause expensive work when the
    /// screen turns off. May report optimistic defaults on platforms
    /// without PowerDevil/upower/logind (e.g. screenActive stays true), so
    /// callers should treat "screen off" as a definite signal and "screen
    /// on" as the safe default.
    [[nodiscard]] Core::PowerManager           *powerManager()   const;

    /// V32.34+ battery: the shared sleep-state machine. Non-null after
    /// initialize(). UI/network components subscribe to its
    /// sleepStateChanged signal to gate expensive work (animations,
    /// camera, ambient derive) on one coherent state instead of each
    /// interpreting raw screen/app/battery signals independently.
    [[nodiscard]] Core::SleepStateMachine      *sleepStateMachine() const;

    /// V32.32: process-wide face library, used by Phase 1 of the face-aware
    /// bandwidth optimization (see docs/FACE_BANDWIDTH.md). Always non-null
    /// after `initialize()` returns — when the Rust face crate isn't compiled
    /// in, returns a Library instance whose methods report NotCompiled and
    /// safely degrade to "no-op." Callers can use it unconditionally.
    [[nodiscard]] Face::Library               *faceLibrary()     const;

    /// V32.32: process-wide face detector. May be null when no model
    /// file was found at startup OR when the Rust face crate isn't
    /// compiled in. Callers MUST null-check before calling; null
    /// detector means "no face detection available, skip the feature."
    /// (Unlike Library, which always returns non-null and degrades
    /// gracefully — Detector's heaviness justifies the asymmetry.)
    [[nodiscard]] Face::Detector              *faceDetector()    const;

    /// Wipe all peer data, identity, and DB then quit so the user can
    /// restart with a clean slate. Call from UI reset button.
    Q_INVOKABLE void factoryReset();

    /// Clear all cached peer addresses & DHT lookup results.
    /// Forces the next connection attempt to re-resolve via DHT.
    /// Useful when peer addresses are stale (peer moved networks)
    /// without doing a full factory reset.
    void clearAddressCache();
    [[nodiscard]] Crypto::ICryptoService      *cryptoService()   const;
    [[nodiscard]] SessionManager              *sessionManager()  const;
    [[nodiscard]] SnapRouter                  *snapRouter()       const;
    /// V32.21: app-state snapshot for adaptive network decisions.
    /// Always non-null after initialize() returns. Consumers either call
    /// `context()` for the current snapshot or wire to the
    /// `contextChanged` signal for live updates. See core/AppContext.hpp.
    [[nodiscard]] AppContextProvider           *appContext()      const;

    // ── User-facing commands (called by UI, executed by subsystems) ───────
    void sendSnap(const Snap &snap);

    /// Send a location-only snap to a peer (no photo).
    /// Uses the most recent location fix from LocationManager. If no fix
    /// is available (or location sharing is disabled in settings), this
    /// returns false and emits no snap.
    /// `caption` is optional — appears as the snap's overlay text.
    bool sendLocationSnap(const Core::UserId &recipientId,
                          const QString      &caption = {});
    void markSnapViewed(const SnapId &id);
    void addFriend(const UserId &friendId, const PeerAddress &address);
    // Resolve a DHT stable ID to a WS address then connect.
    // If already known, connects immediately; otherwise does a DHT lookup.
    void connectByDhtId(const QString &dhtId);
    void tryNextPeerAddress(const QString &peerId);
    void approveInboundPeer(const Core::UserId &peerId);
    void denyInboundPeer(const Core::UserId &peerId);
    [[nodiscard]] std::optional<Core::PeerLocation>
        lastKnownLocation(const Core::UserId &peerId) const;

    /// V32.47: call-aware backoff. The outbound voice path (MainWindow) calls
    /// noteVoiceActivity() when it sends a voice frame; the inbound path marks it
    /// internally. callActive() is true while a call is live, and the background
    /// reconnection loop quiets down when it is.
    void noteVoiceActivity();
    [[nodiscard]] bool callActive() const;

signals:
    void snapReceived(SnapClone::Core::Snap snap);
    void snapDelivered(SnapClone::Core::SnapId id);

    /// V32.39+: an inbound group-voice packet (VoiceWire-enveloped) from a peer.
    /// Routed to the open conversation's voice call (via InboxWidget). The
    /// envelope demux happens in the inbound payload dispatch.
    void voicePayloadReceived(SnapClone::Core::UserId sender, QByteArray payload);
    /// Snap saved to outbox; peer offline. Triggers reconnect; SYNC delivers it.
    void snapQueued(SnapClone::Core::SnapId id, SnapClone::Core::UserId recipientId);
    void peerConnected(SnapClone::Core::UserId userId);
    void peerDisconnected(SnapClone::Core::UserId userId);
    void errorOccurred(QString message);
    // Emitted at each phase of the connection attempt so the UI can show live status.
    // phase: "connecting" | "punching" | "mailbox" | "relookup" | "udp" | "connected" | "failed" | ""
    // detail: short human-readable string describing current action
    void peerConnectionStatus(SnapClone::Core::UserId userId,
                               QString phase, QString detail);
    // Emitted when device-level network connectivity changes.
    // online=true: we have a usable network interface again.
    // online=false: all interfaces gone (airplane mode, WiFi off, etc.)
    // The UI should show a global offline banner and stop showing
    // per-peer "connecting" spinners when online=false.
    void networkReachabilityChanged(bool online);

    // Emitted when the device switches SSID / network (e.g. home WiFi → hotspot).
    // oldIp may be empty if we were offline. newIp is the new primary IPv4.
    void localNetworkChanged(QString oldIp, QString newIp);

    // Emitted when a notable network-layer event happens for a peer.
    // stat: short one-line human-readable transport/address string, e.g.
    //   "UDP  166.205.208.145:64033"  "WebSocket  69.156.40.205:9876"  "Relay"
    // The UI displays this in a per-card stat line, refreshed every 3 s with a fade.
    void peerStatUpdate(SnapClone::Core::UserId userId, QString stat);
    // Emitted when an unknown peer connects inbound and wants to add us.
    // UI should show an approve/deny prompt; call approveInboundPeer / denyInboundPeer.
    void inboundConnectionRequest(SnapClone::Core::UserId peerId,
                                   QString displayName);
    /// Emitted when a snap's view timer expires — UI should close the viewer.
    void snapViewExpired(SnapClone::Core::SnapId id);
    /// Emitted when a peer's location is learned from a received snap.
    void peerLocationReceived(SnapClone::Core::UserId id,
                               SnapClone::Core::PeerLocation loc);

private:
    void sendShak(const Core::UserId &peerId, int attempt = 0);

    // V32.33+ PQ Phase C: SHAK_v2 with hybrid post-quantum KEM material.
    //
    // Sent ALONGSIDE the legacy SHAK during the transition period — older
    // peers ignore the unknown magic and use only the v1 packet, newer
    // peers see both and prefer v2. Once enough peers have rolled forward,
    // v1 emission becomes a fallback (Phase E).
    //
    // SHAK_v2 carries an ephemeral ML-KEM-768 encapsulation key (1184B)
    // when sent from the INITIATOR side, OR an ML-KEM-768 ciphertext
    // (1088B) when sent from the RESPONDER side. The protocol owner
    // (m_pendingMlkemDk in Impl) holds the matching decapsulation key
    // until the responder's reply arrives.
    //
    // peerX25519Pub is REQUIRED when asResponder=true (we encapsulate
    // against peerMlkemEk and also compute X25519 ECDH against the
    // peer's pubkey). Initiators set it to empty.
    void sendShakV2(const Core::UserId &peerId,
                    bool asResponder = false,
                    const QByteArray &peerMlkemEk = {},
                    const QByteArray &peerX25519Pub = {},
                    int attempt = 0);

    // Wire signal/slot connections between subsystems.
    void connectSubsystems();

    // Network reachability event handlers — called from QNetworkInformation
    // callback or poll timer when connectivity changes are detected.
    void onNetworkWentOffline();
    void onNetworkCameOnline();
    void onNetworkIpChanged(const QString &newIp);

    struct Impl;
    std::unique_ptr<Impl> d; ///< Pimpl: insulates UI translation units from subsystem headers.
};

} // namespace SnapClone::Core
