// ─── Network/JamiAdapter.cpp ──────────────────────────────────────────────────
//
// Jami DHT backend integration.
//
// libjami version compatibility:
//   This file targets the modern libjami API (jami-daemon >= 20230000).
//   The old "calls for data channels" approach (libjami_init / placeCall /
//   sendTextMessage) has been replaced with the Conversation + DataTransfer API:
//
//   Old (deprecated):
//     libjami_init() / libjami_start() / libjami_fini()
//     placeCallWithMedia() to establish data channels
//     sendTextMessage() with base64 payload
//
//   New (current):
//     libjami::init()       / libjami::start()     / libjami::fini()
//     libjami::sendMessage() via Conversation API  (text + binary)
//     libjami::sendFile()    via DataTransfer API  (large binary)
//
// Architecture:
//   JamiDiscovery → IDiscovery
//     start()            — init libjami, load/create account
//     announcePresence() — set display name in DHT
//     lookupPeer()       — DHT name lookup by UserId string
//     ↳ peerDiscovered() — emits virtual jami:// PeerAddress for NetworkManager
//
//   JamiTransport → ITransport
//     Wraps a Jami conversation for binary snap delivery.
//     send()     — sendMessage() with content-type = "application/snapclone"
//     received() — messageReceived() callback decoded from base64
//
// Dual backend:
//   AppController creates BOTH a GossipDiscovery (LAN) and a JamiDiscovery
//   (internet).  NetworkManager takes the primary discovery; the secondary
//   is attached separately.  Both emit peerDiscovered to the same slot.
//
// Thread safety:
//   All libjami callbacks arrive on libjami's internal threads (ThreadLoop).
//   We marshal every callback to the Qt main thread via invokeMethod with
//   Qt::QueuedConnection before touching any Qt objects.
// ─────────────────────────────────────────────────────────────────────────────

#include "network/JamiAdapter.hpp"
#include <QLoggingCategory>
#include <QStandardPaths>
#include <QDir>
#include <QByteArray>

Q_LOGGING_CATEGORY(lcJami, "snapclone.jami")

#ifdef SNAPCLONE_JAMI_BACKEND
// Modern libjami public API headers (jami-daemon >= 20230000).
// Install: apt install libjami-dev  /  apk add jami-daemon-dev
#  include <jami/jami.h>
#  include <jami/account_const.h>
#  include <jami/configurationmanager_interface.h>
#  include <jami/callmanager_interface.h>
#  include <jami/conversationmanager_interface.h>
#  include <jami/datatransfer_interface.h>
#  include <jami/presencemanager_interface.h>
#endif

namespace SnapClone::Network {

// ─── Pimpl ────────────────────────────────────────────────────────────────────

struct JamiDiscovery::Impl {
    QString         accountId;
    JamiAccountInfo info;
    bool            initialised = false;
    bool            started     = false;
};

struct JamiTransport::Impl {
    bool    connected      = false;
    QString conversationId; // Jami conversation ID for this peer
};

// ─── JamiDiscovery ───────────────────────────────────────────────────────────

JamiDiscovery::JamiDiscovery(QObject *parent)
    : IDiscovery(parent)
    , d(std::make_unique<Impl>())
{}

JamiDiscovery::~JamiDiscovery()
{
    stop();
}

bool JamiDiscovery::isAvailable()
{
#ifdef SNAPCLONE_JAMI_BACKEND
    return true;
#else
    return false;
#endif
}

std::optional<JamiAccountInfo> JamiDiscovery::accountInfo() const
{
    if (d->info.accountId.isEmpty()) return std::nullopt;
    return d->info;
}

void JamiDiscovery::start()
{
    if (d->started) return;
    d->started = true;

#ifdef SNAPCLONE_JAMI_BACKEND
    qCInfo(lcJami) << "Initialising libjami…";

    const QString configDir =
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation)
        + QStringLiteral("/jami");
    QDir().mkpath(configDir);

    // Modern libjami::init() — replaces deprecated libjami_init().
    // LIBJAMI_FLAG_NO_LOCAL_MEDIA: we only use data channels, not audio/video.
    // This avoids pulling in PulseAudio/PipeWire audio dependencies.
    using namespace libjami;
    const InitFlag flags = InitFlag(LIBJAMI_FLAG_NO_LOCAL_MEDIA
                                  | LIBJAMI_FLAG_CONSOLE_LOG);

    if (!init(flags)) {
        qCCritical(lcJami) << "libjami::init() failed";
        d->started = false;
        return;
    }

    registerCallbacks();

    if (!start()) {
        qCCritical(lcJami) << "libjami::start() failed";
        fini();
        d->started = false;
        return;
    }

    d->initialised = true;

    // Load the first existing account, or create one.
    const auto accounts = getAccountList();
    if (!accounts.empty()) {
        d->accountId = QString::fromStdString(accounts.front());
        qCInfo(lcJami) << "Loaded existing Jami account:" << d->accountId;
        // Re-enable it in case it was marked inactive.
        setAccountActive(d->accountId.toStdString(), true, false);
    } else {
        createAccount();
    }
#else
    qCInfo(lcJami) << "Jami backend not compiled in (SNAPCLONE_JAMI_BACKEND=OFF).";
    qCInfo(lcJami) << "Rebuild with: cmake -DSNAPCLONE_JAMI_BACKEND=ON";
#endif
}

void JamiDiscovery::stop()
{
    if (!d->started) return;
    d->started = false;

#ifdef SNAPCLONE_JAMI_BACKEND
    if (d->initialised) {
        qCInfo(lcJami) << "Shutting down libjami…";
        libjami::fini(); // modern API replaces libjami_fini()
        d->initialised = false;
    }
#endif
}

void JamiDiscovery::announcePresence(const Core::UserId &self,
                                      const Core::PeerAddress & /*addr*/)
{
#ifdef SNAPCLONE_JAMI_BACKEND
    if (d->accountId.isEmpty()) return;

    // Register our SnapClone username (e.g. "kesslerd@192.168.2.178") as the
    // Jami display name so remote peers can find us by this identity string.
    std::map<std::string, std::string> details;
    details[libjami::Account::ConfProperties::DISPLAY_NAME] = self.value.toStdString();
    details[libjami::Account::ConfProperties::ALIAS]        = self.value.toStdString();
    libjami::setAccountDetails(d->accountId.toStdString(), details);

    qCInfo(lcJami) << "Registered Jami presence as:" << self.value;
#else
    Q_UNUSED(self)
#endif
}

void JamiDiscovery::lookupPeer(const Core::UserId &id)
{
#ifdef SNAPCLONE_JAMI_BACKEND
    if (d->accountId.isEmpty()) return;

    qCInfo(lcJami) << "DHT lookup for:" << id.value;

    // lookupName() posts an async OpenDHT query for the display name.
    // The result arrives in the RegisteredNameFound callback → onJamiNameLookupResult.
    // Empty string for nameserver = use the default Jami nameserver.
    libjami::lookupName(d->accountId.toStdString(), "", id.value.toStdString());
#else
    Q_UNUSED(id)
#endif
}

// ─── libjami callback registration ───────────────────────────────────────────

void JamiDiscovery::registerCallbacks()
{
#ifdef SNAPCLONE_JAMI_BACKEND
    using namespace libjami;

    // ── Account state: fired when registration on DHT changes ──────────────
    auto accountCb = exportable_callback<ConfigurationSignal::AccountsChanged>(
        [this]() {
            QMetaObject::invokeMethod(this, [this]() {
                const auto accts = libjami::getAccountList();
                if (!accts.empty() && d->accountId.isEmpty()) {
                    d->accountId = QString::fromStdString(accts.front());
                    qCInfo(lcJami) << "Account registered:" << d->accountId;
                }
            }, Qt::QueuedConnection);
        });

    // ── Account registration state ──────────────────────────────────────────
    auto regCb = exportable_callback<ConfigurationSignal::VolatileDetailsChanged>(
        [this](const std::string &accountId,
               const std::map<std::string, std::string> &details) {
            const QString qId = QString::fromStdString(accountId);
            const auto stateIt = details.find(
                libjami::Account::VolatileProperties::REGISTRATION_STATE);
            if (stateIt == details.end()) return;
            const QString state = QString::fromStdString(stateIt->second);
            QMetaObject::invokeMethod(this, [this, qId, state]() {
                onJamiAccountStateChanged(qId, state);
            }, Qt::QueuedConnection);
        });

    // ── DHT name lookup result ──────────────────────────────────────────────
    auto lookupCb = exportable_callback<ConfigurationSignal::RegisteredNameFound>(
        [this](const std::string &accountId, int status,
               const std::string &address, const std::string &name) {
            const QString qAcct    = QString::fromStdString(accountId);
            const QString qAddress = QString::fromStdString(address);
            const QString qName    = QString::fromStdString(name);
            QMetaObject::invokeMethod(this, [this, qAcct, status, qAddress, qName]() {
                onJamiNameLookupResult(qAcct, status, qName, qAddress);
            }, Qt::QueuedConnection);
        });

    // ── Inbound conversation request (peer wants to connect) ────────────────
    // Modern Jami: peers connect by sending a conversation request, not a call.
    auto convReqCb = exportable_callback<ConversationSignal::ConversationRequestReceived>(
        [this](const std::string &accountId,
               const std::string &conversationId,
               const std::map<std::string, std::string> &metadata) {
            const QString qAcct = QString::fromStdString(accountId);
            const QString qConv = QString::fromStdString(conversationId);
            // Auto-accept all SnapClone conversation requests.
            QMetaObject::invokeMethod(this, [this, qAcct, qConv]() {
                onConversationRequestReceived(qAcct, qConv);
            }, Qt::QueuedConnection);
            Q_UNUSED(metadata)
        });

    // ── Inbound message on a conversation ───────────────────────────────────
    auto msgCb = exportable_callback<ConversationSignal::MessageReceived>(
        [this](const std::string &accountId,
               const std::string &conversationId,
               std::map<std::string, std::string> message) {
            // Only handle our binary snap payloads.
            const auto typeIt = message.find("application/snapclone");
            if (typeIt == message.end()) return;

            const QByteArray payload =
                QByteArray::fromBase64(QByteArray::fromStdString(typeIt->second));
            const QString qAcct = QString::fromStdString(accountId);
            const QString qConv = QString::fromStdString(conversationId);
            const auto fromIt   = message.find("from");
            const QString qFrom = (fromIt != message.end())
                ? QString::fromStdString(fromIt->second) : QString{};

            QMetaObject::invokeMethod(this,
                [this, qAcct, qConv, qFrom, payload]() {
                    onJamiMessageReceived(qAcct, qConv, qFrom, payload);
                }, Qt::QueuedConnection);
        });

    registerSignalHandlers({
        exported_callback<ConfigurationSignal::AccountsChanged>(std::move(accountCb)),
        exported_callback<ConfigurationSignal::VolatileDetailsChanged>(std::move(regCb)),
        exported_callback<ConfigurationSignal::RegisteredNameFound>(std::move(lookupCb)),
        exported_callback<ConversationSignal::ConversationRequestReceived>(
            std::move(convReqCb)),
        exported_callback<ConversationSignal::MessageReceived>(std::move(msgCb)),
    });

    qCDebug(lcJami) << "libjami callbacks registered.";
#endif
}

// ─── Account management ───────────────────────────────────────────────────────

void JamiDiscovery::createAccount()
{
#ifdef SNAPCLONE_JAMI_BACKEND
    qCInfo(lcJami) << "Creating new Jami account…";

    std::map<std::string, std::string> details;
    details[libjami::Account::ConfProperties::TYPE]         = "RING";
    details[libjami::Account::ConfProperties::DISPLAY_NAME] = "SnapClone";
    // UPnP and NAT-PMP for automatic port mapping (dhtnet::upnp::PUPnP).
    details[libjami::Account::ConfProperties::UPNP_ENABLED] = "true";
    // Allow peer-to-peer without a relay when possible (ICE direct path).
    details[libjami::Account::ConfProperties::TURN_ENABLED] = "true";

    const std::string newId = libjami::addAccount(details);
    if (newId.empty()) {
        qCCritical(lcJami) << "addAccount() returned empty ID.";
        return;
    }
    d->accountId = QString::fromStdString(newId);
    qCInfo(lcJami) << "New Jami account:" << d->accountId;
#endif
}

// ─── Slot implementations ─────────────────────────────────────────────────────

void JamiDiscovery::onJamiAccountStateChanged(const QString &accountId,
                                               const QString &state)
{
    qCInfo(lcJami) << "Account" << accountId << "→" << state;
#ifdef SNAPCLONE_JAMI_BACKEND
    if (accountId != d->accountId) return;
    if (state != QStringLiteral("REGISTERED")) return;

    const auto details = libjami::getAccountDetails(accountId.toStdString());

    auto get = [&](const std::string &key) -> QString {
        const auto it = details.find(key);
        return (it != details.end()) ? QString::fromStdString(it->second) : QString{};
    };

    d->info.accountId   = accountId;
    d->info.jamiId      = get(libjami::Account::ConfProperties::USERNAME);
    d->info.displayName = get(libjami::Account::ConfProperties::DISPLAY_NAME);
    d->info.registered  = true;

    qCInfo(lcJami) << "Jami ID:" << d->info.jamiId
                   << "Display:" << d->info.displayName;
    emit jamiAccountReady(d->info);
#endif
}

void JamiDiscovery::onJamiNameLookupResult(const QString & /*accountId*/,
                                             int status,
                                             const QString &name,
                                             const QString &jamiId)
{
    if (status != 0 || jamiId.isEmpty()) {
        qCWarning(lcJami) << "DHT lookup for" << name
                          << "failed (status=" << status << ")";
        return;
    }

    qCInfo(lcJami) << "DHT found" << name << "→ Jami ID:" << jamiId;

    // Expose as a jami:// virtual address. NetworkManager creates a session
    // keyed on UserId{name}; JamiTransport handles the actual delivery.
    const Core::PeerAddress addr{ QStringLiteral("jami://") + jamiId, 0 };
    emit peerDiscovered(Core::UserId{ name }, addr);
}

void JamiDiscovery::onConversationRequestReceived(const QString &accountId,
                                                    const QString &conversationId)
{
#ifdef SNAPCLONE_JAMI_BACKEND
    qCInfo(lcJami) << "Inbound conversation request:" << conversationId;
    // Auto-accept: all SnapClone users who find us on DHT are legitimate peers.
    libjami::acceptConversationRequest(accountId.toStdString(),
                                       conversationId.toStdString());

    // We don't know the peer's UserId yet — wait for the first message
    // which carries the sender's display name via the "from" field.
    Q_UNUSED(conversationId)
#else
    Q_UNUSED(accountId)
    Q_UNUSED(conversationId)
#endif
}

void JamiDiscovery::onJamiMessageReceived(const QString &accountId,
                                            const QString &conversationId,
                                            const QString &fromJamiId,
                                            const QByteArray &payload)
{
    Q_UNUSED(accountId)
    qCDebug(lcJami) << "Snap payload received via Jami from" << fromJamiId
                    << "conv=" << conversationId << "size=" << payload.size();

    // Map the sender's Jami ID to a transport-level address and emit
    // a peerDiscovered so NetworkManager creates/finds a session.
    const Core::PeerAddress addr{ QStringLiteral("jami://") + fromJamiId, 0 };
    const Core::UserId peerId{ fromJamiId };
    emit peerDiscovered(peerId, addr);

    // Forward the raw payload upward as if it arrived on a WebSocket.
    emit jamiPayloadReceived(peerId, payload, conversationId);
}

// ─── JamiTransport ───────────────────────────────────────────────────────────
//
// Wraps a Jami conversation to provide the ITransport interface.
// Binary snap payloads are base64-encoded in the "application/snapclone"
// MIME type of a Jami message.  This keeps the payload opaque to Jami's
// own encryption layer — the snap is double-encrypted (our XOR/AES on
// top of Jami's TLS + libsodium).

JamiTransport::JamiTransport(const QString &accountId,
                              const QString &conversationId,
                              QObject *parent)
    : ITransport(parent)
    , m_accountId(accountId)
    , m_callId(conversationId)
    , d(std::make_unique<Impl>())
{
    d->conversationId = conversationId;
}

JamiTransport::~JamiTransport()
{
    disconnect();
}

void JamiTransport::connectToPeer(const Core::PeerAddress &address)
{
#ifdef SNAPCLONE_JAMI_BACKEND
    // Extract the Jami ID from the "jami://HEXID" virtual address.
    const QString jamiId = address.host.startsWith(QStringLiteral("jami://"))
                           ? address.host.mid(7)
                           : address.host;

    qCInfo(lcJami) << "Starting Jami conversation with" << jamiId;

    // Create or find an existing conversation with this peer.
    // libjami::startConversation() returns a conversation ID immediately;
    // the peer gets a ConversationRequestReceived callback.
    const std::string convId =
        libjami::startConversation(m_accountId.toStdString());

    if (convId.empty()) {
        qCWarning(lcJami) << "startConversation() returned empty ID.";
        emit errorOccurred(QStringLiteral("Failed to start Jami conversation."));
        return;
    }

    d->conversationId = QString::fromStdString(convId);
    m_callId          = d->conversationId;

    // Invite the peer to the conversation.
    libjami::addConversationMember(m_accountId.toStdString(),
                                   d->conversationId.toStdString(),
                                   jamiId.toStdString());

    // We optimistically mark as connected — Jami conversations don't
    // have an explicit "accepted" callback in the same way calls do.
    // The first successful sendMessage confirms connectivity.
    d->connected = true;
    emit connected();
    qCInfo(lcJami) << "Jami conversation initiated:" << d->conversationId;
#else
    Q_UNUSED(address)
    emit errorOccurred(QStringLiteral("Jami backend not compiled in."));
#endif
}

void JamiTransport::disconnect()
{
    // Jami conversations persist in the DHT — we don't "hang up".
    // Just mark as disconnected on our side.
    if (d->connected) {
        d->connected = false;
        emit disconnected();
    }
}

void JamiTransport::send(const QByteArray &payload)
{
    if (!d->connected || d->conversationId.isEmpty()) {
        qCWarning(lcJami) << "send() on disconnected JamiTransport";
        return;
    }

#ifdef SNAPCLONE_JAMI_BACKEND
    // Base64-encode the binary snap payload for the Jami message body.
    // We use the custom MIME type so the receiver knows to decode it
    // and not display it as a regular text message.
    const std::string encoded = payload.toBase64().toStdString();
    const std::map<std::string, std::string> message = {
        { "application/snapclone", encoded }
    };

    libjami::sendMessage(m_accountId.toStdString(),
                         d->conversationId.toStdString(),
                         encoded,
                         "" /* parent message id */,
                         false /* not a flag */);

    qCDebug(lcJami) << "Snap sent via Jami, encoded size ="
                    << static_cast<int>(encoded.size());
#else
    Q_UNUSED(payload)
    emit errorOccurred(QStringLiteral("Jami backend not compiled in."));
#endif
}

bool JamiTransport::isConnected() const
{
    return d->connected;
}

void JamiTransport::onJamiCallStateChanged(const QString &callId,
                                             const QString &state)
{
    // Kept for API compatibility — not used in conversation-based transport.
    Q_UNUSED(callId)
    Q_UNUSED(state)
}

void JamiTransport::onJamiDataReceived(const QString &callId,
                                         const QByteArray &data)
{
    if (callId != d->conversationId) return;
    emit messageReceived(data);
}

} // namespace SnapClone::Network

// ─── OpenDHT direct backend ──────────────────────────────────────────────────
//
// When SNAPCLONE_OPENDHT_BACKEND is defined (and SNAPCLONE_JAMI_BACKEND is not),
// we use dhtnet for peer discovery and NAT traversal.
//
// dhtnet is the networking layer extracted from Jami.  It provides:
//   • OpenDHT for decentralised peer discovery (same DHT Jami uses)
//   • ICE/STUN/TURN hole-punching via pjnath — works across NAT
//   • TLS peer connections via GnuTLS + Ed25519 certificates
//   • UPnP/NAT-PMP automatic port mapping
//
// Discovery strategy (same as raw opendht approach):
//   Each peer publishes its WebSocket endpoint (host:port) as a signed DHT
//   value under the key SHA-1(userId).  Peers look up that key → get the
//   address → connect via the existing WebSocket/SnapRouter pipeline.
//   DHT is used for discovery only; snaps travel over WebSocket as normal.
//
// Future improvement: use dhtnet::ConnectionManager directly for ICE-based
// connections that work even when both peers are behind strict NAT.  The
// infrastructure is there; it requires replacing WebSocketTransport with a
// dhtnet::ChannelSocket-based transport for the ICE-discovered path.
//
// Submodule: third_party/dhtnet (https://github.com/savoirfairelinux/dhtnet)
// Build deps: apk add python3 gnutls-dev nettle-dev argon2-dev fmt-dev

// DhtDiscovery implementation is in JamiAdapter_dht_impl.cpp (compiled separately).
// Splitting it avoids including opendht_c.h here which would require opendht
// on the include path for the entire network module during non-DHT builds.