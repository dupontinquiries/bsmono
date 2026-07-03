#include "network/UdpChannel.hpp"
#include "defense/FSMValidator.hpp"
#include "debug/DebugFlags.hpp"
#include "core/AppContext.hpp"  // V32.28: context-adaptive retransmit cadence
#include <QLoggingCategory>
#include <QDataStream>
#include <QRandomGenerator>
#include <QDateTime>

Q_LOGGING_CATEGORY(lcUdp, "snapclone.udpchan")

namespace SnapClone::Network {

UdpChannel::UdpChannel(QObject *parent)
    : QObject(parent)
{
    m_retransmitTimer.setInterval(kAckTimeoutMs);
    connect(&m_retransmitTimer, &QTimer::timeout,
            this, &UdpChannel::onRetransmitTimer);

    m_keepaliveTimer.setInterval(kKeepaliveMs);
    connect(&m_keepaliveTimer, &QTimer::timeout,
            this, &UdpChannel::onKeepaliveTimer);
}

UdpChannel::~UdpChannel() { stop(); }

void UdpChannel::start(const Core::UserId &selfId, WriteFn writeFn)
{
    m_selfId  = selfId;
    m_writeFn = std::move(writeFn);
    // Timers start lazily when first peer connects
    qCInfo(lcUdp) << "UdpChannel started (using punch socket for I/O)";
}

void UdpChannel::stop()
{
    m_retransmitTimer.stop();
    m_keepaliveTimer.stop();
}

void UdpChannel::connectToPeer(const Core::UserId     &peerId,
                                const Core::PeerAddress &peerAddr)
{
    // Self-loopback guard. Mirrors the NatTraversal-side guard. Without
    // this, a SCUD CONNECT to 127.0.0.1 would write back into our own
    // socket, get re-delivered as inbound, we'd reply with CONNECT_ACK
    // also to 127.0.0.1, infinite loop. The receive-side guard in
    // NatTraversal::onPunchReadyRead drops these on intake; this guard
    // refuses to even queue them on send.
    if (peerAddr.host == QLatin1String("127.0.0.1")
     || peerAddr.host == QLatin1String("::1")
     || peerAddr.host.compare(QLatin1String("localhost"), Qt::CaseInsensitive) == 0) {
        qCWarning(lcUdp) << "connectToPeer: refusing self-loopback target"
                         << peerAddr.toString() << "for" << peerId.value;
        return;
    }
    auto &peer = m_peers[peerId.value];
    // If a transfer was in-flight for the old address, cancel it.
    // The retransmit timer would otherwise send stale data to the new address.
    if (!peer.sendPayload.isEmpty()) {
        qCInfo(lcUdp) << "connectToPeer: cancelling in-flight transfer for"
                      << peerId.value << "(address change)";
        peer.sendPayload.clear();
        peer.sendNext     = 0;
        peer.windowBase   = 0;
        peer.ackedChunks.clear();
    }
    peer.addr      = peerAddr;
    peer.lastSeen  = QDateTime::currentMSecsSinceEpoch();
    peer.connected = false; // must re-handshake on new address
    peer.phase     = Phase::Connecting;  // we're initiating; await CONNECT_ACK
    if (!m_retransmitTimer.isActive()) m_retransmitTimer.start();
    if (!m_keepaliveTimer.isActive()) m_keepaliveTimer.start();
    const QString addrKey = peerAddr.host + QLatin1Char(':')
                          + QString::number(peerAddr.port);
    m_addrToId[addrKey] = peerId.value;

    peer.connectAttempts = 0;
    qCInfo(lcUdp) << "UDP channel: connecting to" << peerId.value
                  << "@" << peerAddr.toString();
    sendFrame(peer, CONNECT, 0, 0, 0);
}

bool UdpChannel::sendPayload(const Core::UserId &peerId,
                              const QByteArray   &payload)
{
    auto it = m_peers.find(peerId.value);
    if (it == m_peers.end() || !it->connected) {
        qCDebug(lcUdp) << "sendPayload: no connected UDP channel for"
                       << peerId.value;
        return false;
    }
    auto &peer = *it;
    if (!peer.sendPayload.isEmpty()) {
        qCWarning(lcUdp) << "sendPayload: transfer already in progress for"
                         << peerId.value << "— replacing";
    }

    peer.sendPayload  = payload;
    peer.sendSession  = static_cast<quint16>(
        QRandomGenerator::global()->generate() & 0xFFFF);
    peer.sendTotal    = static_cast<int>((payload.size() + kChunkSize - 1) / kChunkSize);
    if (peer.sendTotal == 0) peer.sendTotal = 1;
    peer.sendNext          = 0;
    peer.windowBase        = 0;
    peer.ackedChunks.clear();
    peer.chunkRetransmits.clear();

    qCInfo(lcUdp) << "UDP send" << payload.size() << "bytes to" << peerId.value
                  << "in" << peer.sendTotal << "chunk(s), session"
                  << peer.sendSession;
    sendNextChunk(peerId.value, peer);
    return true;
}

bool UdpChannel::isConnected(const Core::UserId &peerId) const
{
    const auto it = m_peers.constFind(peerId.value);
    return it != m_peers.constEnd() && it->connected;
}

// ─── Frame building ───────────────────────────────────────────────────────────

QByteArray UdpChannel::buildFrame(FrameType type, quint16 session,
                                   quint32 seq, quint32 total,
                                   const QByteArray &payload) const
{
    const QByteArray idBytes = m_selfId.value.toUtf8();
    QByteArray frame;
    frame.reserve(4 + 1 + 2 + 4 + 4 + 2 + idBytes.size() + payload.size());

    QDataStream ds(&frame, QIODeviceBase::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << static_cast<quint32>(kMagic)
       << static_cast<quint8>(type)
       << session
       << seq
       << total
       << static_cast<quint16>(idBytes.size());
    ds.writeRawData(idBytes.constData(), idBytes.size());
    if (!payload.isEmpty())
        ds.writeRawData(payload.constData(), payload.size());
    return frame;
}

void UdpChannel::sendFrame(const PeerState &peer, FrameType type,
                            quint16 session, quint32 seq, quint32 total,
                            const QByteArray &payload)
{
    if (!m_writeFn) return;
    const QByteArray frame = buildFrame(type, session, seq, total, payload);
    m_writeFn(frame, peer.addr.host, peer.addr.port);
}

void UdpChannel::sendNextChunk(const QString &peerId, PeerState &peer)
{
    // Sliding window: send up to kWindowSize chunks past windowBase
    // before waiting for ACKs. Greatly improves throughput on lossy links.
    const int windowEnd = peer.windowBase + kWindowSize;
    while (peer.sendNext < peer.sendTotal
           && peer.sendNext < windowEnd
           && !peer.ackedChunks.contains(peer.sendNext)) {
        const int offset = peer.sendNext * kChunkSize;
        const QByteArray chunk = peer.sendPayload.mid(offset, kChunkSize);
        sendFrame(peer, CHUNK,
                  peer.sendSession,
                  static_cast<quint32>(peer.sendNext),
                  static_cast<quint32>(peer.sendTotal),
                  chunk);
        qCDebug(lcUdp) << "Sent chunk" << peer.sendNext
                       << "/" << peer.sendTotal
                       << "to" << peerId
                       << "(" << chunk.size() << "B)";
        ++peer.sendNext;
    }
}

// ─── Receive ─────────────────────────────────────────────────────────────────

void UdpChannel::onDatagram(const QByteArray &raw,
                             const QString    &senderIp,
                             quint16           senderPort)
{
        // Final-line-of-defense loopback drop. The NatTraversal-side filter
        // is the primary defence; this guards against any future code path
        // that might call onDatagram directly without going through
        // NatTraversal's callback. BUILD-MARKER: UDPCHAN-LOOPBACK-DROP-V2
        if (senderIp == QLatin1String("127.0.0.1")
         || senderIp.startsWith(QLatin1String("127."))
         || senderIp == QLatin1String("::1")
         || senderIp == QLatin1String("::ffff:127.0.0.1")
         || senderIp.startsWith(QLatin1String("::ffff:127."))) {
            static qint64 s_lastWarnMs = 0;
            static int s_droppedSinceWarn = 0;
            ++s_droppedSinceWarn;
            const qint64 now = QDateTime::currentMSecsSinceEpoch();
            if (now - s_lastWarnMs > 5000) {
                s_lastWarnMs = now;
                qCWarning(lcUdp) << "UDPCHAN-LOOPBACK-DROP-V2: dropped"
                                 << s_droppedSinceWarn
                                 << "loopback datagrams in last 5s,"
                                 << "most recent from"
                                 << senderIp << ":" << senderPort;
                s_droppedSinceWarn = 0;
            }
            return;
        }
        qCDebug(lcUdp) << "onDatagram from" << senderIp << ":" << senderPort
                      << "size" << raw.size();
        // Fixed header is 4 magic + 1 type + 2 session + 4 seq + 4 total +
        // 2 id_len = 17 bytes. Anything shorter is malformed.
        constexpr qsizetype kFixedHdr = 17;
        if (raw.size() < kFixedHdr) return;

        QDataStream ds(raw);
        ds.setByteOrder(QDataStream::BigEndian);

        quint32 magic;  ds >> magic;
        if (magic != static_cast<quint32>(kMagic)) return;

        quint8  typeRaw; ds >> typeRaw;
        quint16 session; ds >> session;
        quint32 seq;     ds >> seq;
        quint32 total;   ds >> total;
        quint16 idLen;   ds >> idLen;
        if (ds.status() != QDataStream::Ok) return;
        // Cast both sides to qsizetype before subtracting to avoid unsigned
        // underflow if raw.size() == 0 (already rejected above, but the
        // explicit cast survives future refactors).
        constexpr qsizetype kMaxIdLen = 256;
        if (idLen > kMaxIdLen) return;                 // reject absurdly long IDs
        if (static_cast<qsizetype>(idLen) > raw.size() - kFixedHdr) return;

        // Sanity: reject implausible total / seq combinations. seq must be
        // less than total for CHUNK frames; non-CHUNK frames set total to 0
        // (already checked in the switch below). Prevents division-by-zero
        // or negative-progress arithmetic later. seq is uint32 so no overflow
        // concerns; total cap protects against attackers claiming a 4GB
        // payload to make us allocate insane buffers.
        constexpr quint32 kMaxChunks = 1u << 20;  // 1M chunks × 48KB = 48GB ceiling
        if (total > kMaxChunks) return;
        if (total > 0 && seq >= total) return;

        QByteArray idBytes(idLen, Qt::Uninitialized);
        ds.readRawData(idBytes.data(), idLen);
        const QString senderId = QString::fromUtf8(idBytes);

        const int hdrSize = kFixedHdr + idLen;
        const QByteArray payload = raw.mid(hdrSize);

        const QString addrKey = senderIp + QLatin1Char(':')
                               + QString::number(senderPort);
        if (!senderId.isEmpty() && !m_addrToId.contains(addrKey)) {
            // BUG E: cap map to prevent unbounded growth from CGNAT port cycling
            constexpr int kMaxAddrMap = 256;
            if (m_addrToId.size() >= kMaxAddrMap) {
                // Evict the first (oldest) entry
                m_addrToId.erase(m_addrToId.begin());
            }
            m_addrToId[addrKey] = senderId;
        }

        const QString peerId = m_addrToId.value(addrKey, senderId);
        if (peerId.isEmpty()) return;

        auto &peer = m_peers[peerId];
        peer.lastSeen = QDateTime::currentMSecsSinceEpoch();
        if (peer.addr.host.isEmpty())
            peer.addr = { senderIp, senderPort };

        const auto type = static_cast<FrameType>(typeRaw);

        // ── FSM alignment check (gated by DebugFlags) ──────────────────────
        // Validates that frame type matches what's legal given peer's phase.
        //
        // When `NoMonitoringBlocking` is OFF (default):
        //   • Misalignments are LOGGED via rate-limited validator
        //   • Frame is still accepted (current "accept everything" behaviour
        //     pending the data we gather from the log)
        //
        // When `NoMonitoringBlocking` is ON (debug):
        //   • Misalignments are NOT logged (silent path)
        //   • Frame is accepted as before
        //
        // When `VerboseFsm` is ON (debug):
        //   • Every transition is logged, not just bad ones
        //
        // Repeated misalignments from one peer are a strong signal of either
        // a protocol bug on our side, a peer-side bug, or an active attack.
        const bool fsmAligned = isLegalInbound(peer.phase, type);
        if (Debug::isDebug(Debug::DebugFlag::VerboseFsm)) {
            qCDebug(lcUdp) << "FSM" << phaseName(peer.phase) << "+"
                           << frameTypeName(type)
                           << (fsmAligned ? "OK" : "MISALIGNED")
                           << "from" << peerId;
        }
        if (!fsmAligned
         && !Debug::isDebug(Debug::DebugFlag::NoMonitoringBlocking)) {
            using namespace SnapClone::Defense;
            // Q_LOGGING_CATEGORY declares lcUdp as a function returning
            // QLoggingCategory& — call it to get the actual reference.
            FSMValidator::logMisalignment(lcUdp(),
                QStringLiteral("UdpChannel"),
                QString::fromLatin1(phaseName(peer.phase)),
                QString::fromLatin1(frameTypeName(type)),
                peerId,
                m_appContext); // V32.28: include AppContext snapshot
            // Continue processing — don't break the wire.
        }

        switch (type) {
        case CONNECT:
            qCInfo(lcUdp) << "UDP CONNECT from" << peerId;
            peer.connected = true;
            peer.phase     = Phase::Established;
            sendFrame(peer, CONNECT_ACK, 0, 0, 0);
            emit channelReady(Core::UserId{ peerId }, peer.addr);
            break;

        case CONNECT_ACK:
            qCInfo(lcUdp) << "UDP CONNECT_ACK from" << peerId
                          << "— channel active";
            peer.connected = true;
            peer.phase     = Phase::Established;
            emit channelReady(Core::UserId{ peerId }, peer.addr);
            break;

        case CHUNK:
            if (!peer.connected) {
                // Peer sent data without CONNECT — accept anyway (FSM logged
                // above if this was a phase mismatch).
                peer.connected = true;
                peer.phase     = Phase::Established;
                sendFrame(peer, CONNECT_ACK, 0, 0, 0);
                emit channelReady(Core::UserId{ peerId }, peer.addr);
            }
            handleChunk(peerId, peer,
                        static_cast<quint16>(session),
                        seq, total, payload);
            break;

        case ACK:
            handleAck(peerId, peer, static_cast<quint16>(session), seq);
            break;

        case KEEPALIVE:
            qCDebug(lcUdp) << "Keepalive from" << peerId;
            // Reply to keep their NAT mapping fresh too
            sendFrame(peer, KEEPALIVE, 0, 0, 0);
            break;

        default:
            break;
        }
}

void UdpChannel::handleChunk(const QString &peerId, PeerState &peer,
                              quint16 session, quint32 seq, quint32 total,
                              const QByteArray &payload)
{
    // ACK this chunk immediately
    sendFrame(peer, ACK, session, seq, 0);

    // Validate total before use — quint32 overflow → negative recvTotal crashes.
    // Cap at 4096 chunks × 8KB = 32MB max per transfer over UDP.
    constexpr quint32 kMaxChunks = 4096;
    if (total == 0 || total > kMaxChunks) {          // BUG B+F: was unchecked
        qCWarning(lcUdp) << "handleChunk: invalid total" << total
                         << "from" << peerId << "— dropping";
        return;
    }
    // New session? Reset reassembly state.
    if (session != peer.recvSession || total != static_cast<quint32>(peer.recvTotal)) {
        peer.recvSession = session;
        peer.recvTotal   = static_cast<int>(total);
        peer.recvChunks.clear();
    }
    // Cap recvChunks to prevent memory exhaustion from duplicate/out-of-order chunks.
    if (peer.recvChunks.size() > static_cast<int>(kMaxChunks)) return;

    peer.recvLastChunkMs = QDateTime::currentMSecsSinceEpoch();
    peer.recvChunks[static_cast<int>(seq)] = payload;
    qCDebug(lcUdp) << "Received chunk" << seq << "/" << total
                   << "from" << peerId
                   << "(" << peer.recvChunks.size() << "so far)";

    // Complete?
    if (peer.recvChunks.size() == peer.recvTotal) {
        QByteArray complete;
        complete.reserve(peer.recvTotal * kChunkSize);
        for (int i = 0; i < peer.recvTotal; ++i)
            complete.append(peer.recvChunks.value(i));
        peer.recvChunks.clear();
        qCInfo(lcUdp) << "UDP payload complete from" << peerId
                      << "(" << complete.size() << "bytes)";
        emit payloadReceived(Core::UserId{ peerId }, complete);
    }
}

void UdpChannel::handleAck(const QString &peerId, PeerState &peer,
                            quint16 session, quint32 seq)
{
    if (session != peer.sendSession) return;
    const int s = static_cast<int>(seq);
    peer.ackedChunks.insert(s);
    peer.chunkRetransmits.remove(s); // no longer needs retransmit
    qCDebug(lcUdp) << "ACK chunk" << seq << "from" << peerId
                   << "(" << peer.ackedChunks.size() << "/" << peer.sendTotal << ")";

    // Advance window base past consecutive ACKed chunks
    while (peer.ackedChunks.contains(peer.windowBase))
        ++peer.windowBase;

    if (peer.ackedChunks.size() == peer.sendTotal) {
        qCInfo(lcUdp) << "Transfer complete to" << peerId;
        peer.sendPayload.clear();
        peer.ackedChunks.clear();
        peer.chunkRetransmits.clear();
        peer.sendNext    = 0;
        peer.windowBase  = 0;
    } else {
        // Advance sendNext past ACKed chunks, then fill the window
        while (peer.sendNext < peer.sendTotal
               && peer.ackedChunks.contains(peer.sendNext))
            ++peer.sendNext;
        sendNextChunk(peerId, peer);
    }
}

// ─── Timers ──────────────────────────────────────────────────────────────────

void UdpChannel::onRetransmitTimer()
{
    ++m_retransmitTickCount;

    // V32.28: skip every other tick when context is conservative
    // (idle + metered/cellular/power-saver) AND the user did NOT just
    // act. Effective cadence becomes 3s instead of 1.5s — saves cellular
    // bytes and battery while pocketed. CONNECT retransmits and
    // user-active sessions stay at full cadence.
    //
    // We DON'T skip when:
    //   - The peer hasn't connected yet (CONNECT retries are time-
    //     sensitive — we need to fail-over to relay in bounded time)
    //   - The user just acted (within last 5s) — they're expecting
    //     snappy delivery, no reason to throttle
    //   - We have no context attached (defensive default)
    bool skipDataRetransmits = false;
    if (m_appContext) {
        const auto &ctx = m_appContext->context();
        if (ctx.prefersConservative()
         && !ctx.userJustActed(5000)
         && (m_retransmitTickCount % 2 == 0)) {
            skipDataRetransmits = true;
        }
    }

    for (auto it = m_peers.begin(); it != m_peers.end(); ++it) {
        auto &peer = it.value();

        // Retransmit CONNECT until we get CONNECT_ACK.
        // Without this, a single lost CONNECT packet means the channel
        // never establishes — one-shot CONNECT is too fragile for CGNAT.
        if (!peer.connected) {
            constexpr int kMaxConnectAttempts = 20; // ~30s of retries
            if (peer.connectAttempts < kMaxConnectAttempts) {
                ++peer.connectAttempts;
                qCDebug(lcUdp) << "Retransmitting CONNECT to" << it.key()
                               << "(attempt" << peer.connectAttempts << ")";
                sendFrame(peer, CONNECT, 0, 0, 0);
            } else if (peer.connectAttempts == kMaxConnectAttempts) {
                // Give up on direct UDP — signal AppController to try relay.
                ++peer.connectAttempts; // prevent repeated emission
                qCWarning(lcUdp) << "UDP CONNECT gave up after" << kMaxConnectAttempts
                                 << "attempts for" << it.key()
                                 << "— signalling for relay fallback";
                emit channelFailed(Core::UserId{ it.key() });
            }
            continue; // don't try chunk retransmit before connected
        }

        if (peer.sendPayload.isEmpty()) continue;

        // V32.28: when conservative+idle and not user-active, halve
        // retransmit cadence by skipping odd ticks for chunk retransmits
        // (CONNECT retransmits above are unaffected — those need full
        // cadence so relay fallback is reached in bounded time).
        if (skipDataRetransmits) continue;

        // Retransmit all unACKed chunks within the current window
        bool aborted = false;
        const int windowEnd = peer.windowBase + kWindowSize;
        for (int i = peer.windowBase; i < qMin(peer.sendTotal, windowEnd); ++i) {
            if (peer.ackedChunks.contains(i)) continue;
            int &retries = peer.chunkRetransmits[i];
            if (++retries > kMaxRetransmit) {
                qCWarning(lcUdp) << "Chunk" << i << "for" << it.key()
                                 << "exceeded" << kMaxRetransmit
                                 << "retransmits — aborting transfer";
                peer.sendPayload.clear();
                peer.ackedChunks.clear();
                peer.chunkRetransmits.clear();
                peer.windowBase = 0;
                peer.sendNext   = 0;
                aborted = true;
                break;
            }
            qCDebug(lcUdp) << "Retransmitting chunk" << i
                           << "to" << it.key()
                           << "(attempt" << retries << ")";
            const int offset = i * kChunkSize;
            const QByteArray chunk = peer.sendPayload.mid(offset, kChunkSize);
            sendFrame(peer, CHUNK, peer.sendSession,
                      static_cast<quint32>(i),
                      static_cast<quint32>(peer.sendTotal), chunk);
        }
        (void)aborted;
    }
}

void UdpChannel::onKeepaliveTimer()
{
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    // Collect dead peers first — emitting channelLost() inside the loop can
    // cause callers to modify m_peers re-entrantly, invalidating the iterator.
    QList<QString> deadPeers;
    for (auto it = m_peers.begin(); it != m_peers.end(); ++it) {
        auto &peer = it.value();
        if (!peer.connected) continue;
        sendFrame(peer, KEEPALIVE, 0, 0, 0);
        if (now - peer.lastSeen > kDeadMs)
            deadPeers.append(it.key());
    }
    for (const QString &pid : deadPeers) {
        if (m_peers.contains(pid)) {
            m_peers[pid].connected = false;
            m_peers[pid].phase     = Phase::Idle;  // FSM reset; next traffic
                                                    // requires fresh handshake
            qCWarning(lcUdp) << "UDP channel dead (no traffic for 60s):" << pid;
        }
        emit channelLost(Core::UserId{ pid });
    }
    // Clear stale partial reassembly maps — a peer that stopped sending mid-transfer
    // (crash, network change) leaves recvChunks growing indefinitely otherwise.
    constexpr qint64 kStaleReassemblyMs = 30000; // 30s with no new chunks
    for (auto &peer : m_peers) {
        if (!peer.recvChunks.isEmpty()
                && peer.recvLastChunkMs > 0
                && now - peer.recvLastChunkMs > kStaleReassemblyMs) {
            qCWarning(lcUdp) << "Clearing stale reassembly buffer";
            peer.recvChunks.clear();
            peer.recvLastChunkMs = 0;
        }
    }
}

} // namespace SnapClone::Network
