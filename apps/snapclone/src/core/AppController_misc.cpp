#include "AppController_p.hpp"
#ifdef SNAPCLONE_RUST_CRYPTO
#include "crypto/RustCryptoService.hpp"
#endif
#include <QDateTime>
#include <QDataStream>
#include <QTimer>
#include <limits>

namespace SnapClone::Core {

void AppController::factoryReset()
{
    qCInfo(lcApp) << "Factory reset requested — wiping all data and identity.";

    // 1. Wipe DB content (peer addresses, contacts, snaps).
    const UserId self{ d->stableId };
    d->storage->purgeAllData(self);

    // 2. Delete the identity file so a new sc-ID is generated on next launch.
    const QString dataDir = QStandardPaths::writableLocation(
        QStandardPaths::AppDataLocation);
    QFile::remove(dataDir + QStringLiteral("/identity.txt"));

    // 3. Wipe QSettings (backend choice, display prefs, etc.).
    QSettings s;
    s.clear();
    s.sync();

    // 4. Quit — the next launch starts clean.
    QCoreApplication::quit();
}

void AppController::approveInboundPeer(const Core::UserId &peerId)
{
    if (!d->pendingInbound.contains(peerId.value)) return;
    d->pendingInbound.remove(peerId.value);
    qCInfo(lcApp) << "[INBOUND] Peer approved:" << peerId.value;

    // Save as a Known contact now that the user accepted.
    Core::Contact c;
    c.id     = peerId;
    c.online = true;
    c.state  = Core::ContactState::Known;
    c.lastSeen = QDateTime::currentDateTimeUtc();
    // Load existing data to preserve nickname/notes/displayName.
    // If no row exists yet, use the displayName we cached in pendingInbound.
    if (const auto existing = d->storage->loadContact(peerId)) {
        c          = *existing;
        c.online   = true;
        c.state    = Core::ContactState::Known;
        c.lastSeen = QDateTime::currentDateTimeUtc();
    } else {
        // Stranger was never saved (rare: SHAK saved Pending but DB wiped between).
        // Recover the display name we stored when the request arrived.
        c.displayName = d->pendingInbound.value(peerId.value, peerId.value);
    }
    d->storage->saveContact(c);

    // Finish the connection flow as if approval was implicit.
    d->relookupCount.remove(peerId.value);
    d->mailboxPublished.remove(peerId.value);
    d->pendingAddresses.remove(peerId.value);
    d->lastAttemptedAddr.remove(peerId.value);
    emit peerConnected(peerId);
    emit peerConnectionStatus(peerId, QString(), QString());
}

void AppController::denyInboundPeer(const Core::UserId &peerId)
{
    d->pendingInbound.remove(peerId.value);
    // Cap deniedPeers to prevent unbounded growth from sustained stranger attacks.
    // FIFO: remove the oldest entry when at capacity.
    // QSet has no ordering, so we use d->deniedPeersOrder for eviction order.
    if (d->deniedPeers.size() >= 512) {
        if (!d->deniedPeersOrder.isEmpty())
            d->deniedPeers.remove(d->deniedPeersOrder.takeFirst());
    }
    d->deniedPeers.insert(peerId.value);
    d->deniedPeersOrder.append(peerId.value);
    qCInfo(lcApp) << "[INBOUND] Peer denied:" << peerId.value;
    // Drop all sessions for this peer.
    if (d->network) d->network->disconnectPeer(peerId);
}


void AppController::sendShak(const Core::UserId &peerId, int attempt)
{
    constexpr int kMaxShakAttempts = 4; // send at T=0, 2s, 4s, 6s then give up
    if (attempt >= kMaxShakAttempts) return;
    if (d->stableId.isEmpty() || !d->crypto) return;
    static constexpr quint32 kShakMagic = 0x5348414B; // "SHAK"
    const Core::UserId selfId{ d->stableId };
    const QByteArray pubKey = d->crypto->publicIdentityKey(selfId);
    if (pubKey.isEmpty()) return;
    const QByteArray idBytes = selfId.value.toUtf8();
    QByteArray msg;
    QDataStream ds(&msg, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kShakMagic << static_cast<quint16>(idBytes.size());
    msg.append(idBytes);
    msg.append(pubKey);
    d->network->sendToPeer(peerId, msg);
    qCInfo(lcApp) << "Sent SHAK (UDP) to" << peerId.value
                  << "(attempt" << attempt + 1 << "of" << kMaxShakAttempts << ")";

    // V32.33+ PQ Phase C: also send SHAK_v2 alongside the legacy SHAK.
    // Peers that don't support v2 ignore the unknown magic. Peers that
    // DO support v2 prefer it (gives them the hybrid PQ session key)
    // and fall back to v1 only if v2 fails. During the transition
    // period, sending both is wasted bandwidth on the v1-only side but
    // ~2.3KB extra on a ~50-byte packet is acceptable for the security
    // gain — and peers we know don't speak v2 (via DHT capability bit,
    // Phase E) can skip the v2 emission.
    //
    // sendShakV2 is a no-op if the crypto backend lacks PQ support
    // (e.g. the Qt-HMAC placeholder build with SNAPCLONE_RUST_CRYPTO=OFF).
    sendShakV2(peerId, /*asResponder=*/false, /*peerMlkemEk=*/QByteArray(),
               /*peerX25519Pub=*/QByteArray(), attempt);

    const QString spid = peerId.value;
    const int nextAttempt = attempt + 1;
    QTimer::singleShot(2000, this, [this, peerId, spid, nextAttempt]() {
        // Retry until THIS session's handshake acks. handshakeRespondedTo is
        // the right signal: it's cleared on disconnect and set on this-
        // session completion. (We deliberately do NOT also stop on
        // pqSessionKeys here — that map persists across reconnects by design,
        // so a stale entry from a prior session would wrongly suppress the
        // reconnect handshake. The ephemeral-reuse fix above already prevents
        // the retry from corrupting an in-flight v2 exchange, which was the
        // actual bug; letting the v1 retry run to completion is correct.)
        if (!d->handshakeRespondedTo.contains(spid))
            sendShak(peerId, nextAttempt);
    });
}

void AppController::sendShakV2(const Core::UserId &peerId, bool asResponder,
                                const QByteArray &peerMlkemEk,
                                const QByteArray &peerX25519Pub,
                                int attempt)
{
    // V32.33+ PQ Phase C: SHAK with hybrid post-quantum KEM material.
    //
    // Wire format (big-endian):
    //   magic           : 4 bytes  "SHK2" (0x53484B32)
    //   version         : 1 byte   0x02
    //   flags           : 1 byte   bit 0 = PQ-enabled, bit 1 = is responder
    //   idLen           : 2 bytes
    //   id              : N bytes  UTF-8
    //   x25519_pub      : 32 bytes
    //   pq_payload_len  : 2 bytes  (either 1184 for ek or 1088 for ct;
    //                               0 if PQ bit is 0)
    //   pq_payload      : N bytes
    //
    // Length-prefixing the PQ payload (instead of inferring from the
    // direction flag) makes the parser self-describing — a future v2.x
    // could grow the payload (e.g. to add ML-DSA signature material)
    // without breaking older v2 parsers.
    //
    // SIGNATURE: not yet added in Phase C. Phase D will append a 64-byte
    // Ed25519 signature over all preceding fields, and update flags
    // bit 2 = "signed". Receivers that see the signed bit but can't
    // verify (no peer Ed25519 key on file yet) must reject.
    constexpr int kMaxShakAttempts = 4;
    if (attempt >= kMaxShakAttempts) return;
    if (d->stableId.isEmpty() || !d->crypto) return;

#ifndef SNAPCLONE_RUST_CRYPTO
    // No Rust crypto backend → no ML-KEM primitives. sendShakV2 is a no-op;
    // the legacy SHAK path handles the placeholder build, and the peer falls
    // back identically. (RustCryptoService isn't even a complete type here.)
    Q_UNUSED(peerId); Q_UNUSED(asResponder);
    Q_UNUSED(peerMlkemEk); Q_UNUSED(peerX25519Pub);
    qCDebug(lcApp) << "sendShakV2: built without SNAPCLONE_RUST_CRYPTO, skipping";
    return;
#else
    // PQ requires the Rust crypto backend. With the Qt placeholder
    // (SNAPCLONE_RUST_CRYPTO=OFF), no ML-KEM primitives are available
    // and we skip v2 entirely — the legacy SHAK is sufficient for the
    // unencrypted-MAC-only placeholder builds, and the receiving side
    // will fall back the same way.
    auto *rustCrypto = dynamic_cast<Crypto::RustCryptoService *>(d->crypto.get());
    if (!rustCrypto || !rustCrypto->pqAvailable()) {
        qCDebug(lcApp) << "sendShakV2: PQ not available, skipping";
        return;
    }

    static constexpr quint32 kShakV2Magic   = 0x53484B32; // "SHK2"
    static constexpr quint8  kShakV2Version = 0x02;
    static constexpr quint8  kFlagPqEnabled = 0x01;
    static constexpr quint8  kFlagIsResponder = 0x02;

    const Core::UserId selfId{ d->stableId };
    const QByteArray ourX25519Pub = rustCrypto->publicIdentityKey(selfId);
    if (ourX25519Pub.isEmpty()) return;
    const QByteArray idBytes = selfId.value.toUtf8();

    // Compute the PQ payload depending on direction.
    QByteArray pqPayload;
    if (asResponder) {
        // Responder side: encapsulate against peer's ml-kem ek, send
        // back the resulting ciphertext.
        if (peerMlkemEk.isEmpty() || peerX25519Pub.isEmpty()) {
            qCWarning(lcApp) << "sendShakV2 responder: missing peer ek or X25519 pub";
            return;
        }
        const auto [ct, sessionKey] =
            rustCrypto->respondHybridHandshake(selfId, peerX25519Pub, peerMlkemEk);
        if (ct.isEmpty() || sessionKey.isEmpty()) {
            qCWarning(lcApp) << "sendShakV2 responder: hybrid handshake failed";
            return;
        }
        // Store the hybrid session key for this peer. Subsequent
        // outbound snaps to this peer will use it instead of (or in
        // addition to, depending on Phase E rollout) the X25519-only key.
        d->pqSessionKeys.insert(peerId.value, sessionKey);
        qCInfo(lcApp) << "sendShakV2 responder: derived hybrid session key for"
                      << peerId.value << "(" << sessionKey.size() << "bytes)";
        pqPayload = ct;
    } else {
        // Initiator side: generate fresh ephemeral ML-KEM keypair, stash
        // the dk, ship the ek.
        //
        // V32.34+ RACE FIX: if we ALREADY have a pending ephemeral for this
        // peer, REUSE it instead of generating a new one. The sendShak()
        // retry loop calls this every 2s until the v1 handshake acks, and
        // each call previously overwrote pendingPqEphemerals[peer] with a
        // fresh keypair — discarding the dk the peer is mid-flight
        // encapsulating against. Sequence that broke:
        //   t0  ship ek1, stash E1
        //   t0+ peer encapsulates ct1 against ek1 (in flight)
        //   t2  retry: ship ek2, stash E2 — E1's dk now GONE
        //   t2+ ct1 arrives → responder path decapsulates with E2's dk
        //       against ct1 → wrong/garbage session key (or finish fails),
        //       and the later ct2 finds no pending ephemeral.
        // Reusing the same ephemeral across retries keeps the dk stable so
        // whichever ct arrives first completes correctly. The ek is already
        // stored in the entry, so re-shipping it is free.
        const auto pit = d->pendingPqEphemerals.constFind(peerId.value);
        if (pit != d->pendingPqEphemerals.constEnd()
            && !pit.value().encapsulationKey.isEmpty()) {
            qCDebug(lcApp) << "sendShakV2: reusing pending ephemeral for"
                           << peerId.value << "(retry — keeping dk stable)";
            pqPayload = pit.value().encapsulationKey;
        } else {
            const auto [dk, ek] = rustCrypto->mlkemGenerateEphemeral();
            if (dk.isEmpty() || ek.isEmpty()) {
                qCWarning(lcApp) << "sendShakV2: failed to generate ephemeral ML-KEM keypair";
                return;
            }
            // Cap the pending map to defend against memory growth.
            constexpr int kMaxPendingPqEphemerals = 64;
            if (d->pendingPqEphemerals.size() >= kMaxPendingPqEphemerals
                && !d->pendingPqEphemerals.contains(peerId.value)) {
                // Find oldest non-self entry and evict it.
                QString oldestKey;
                qint64 oldestMs = std::numeric_limits<qint64>::max();
                for (auto it = d->pendingPqEphemerals.constBegin();
                     it != d->pendingPqEphemerals.constEnd(); ++it) {
                    if (it.value().createdMs < oldestMs) {
                        oldestMs = it.value().createdMs;
                        oldestKey = it.key();
                    }
                }
                if (!oldestKey.isEmpty()) {
                    d->pendingPqEphemerals.remove(oldestKey);
                    qCInfo(lcApp) << "sendShakV2: evicted oldest ephemeral for"
                                  << oldestKey << "(cap" << kMaxPendingPqEphemerals << ")";
                }
            }
            Impl::PqEphemeral entry;
            entry.decapsulationKey = dk;
            entry.encapsulationKey = ek;
            entry.createdMs        = QDateTime::currentMSecsSinceEpoch();
            d->pendingPqEphemerals.insert(peerId.value, entry);
            pqPayload = ek;
        }
    }

    // Build the packet. We use a single QDataStream for the fixed-size
    // header chunks and then directly append the variable-length blobs
    // — appending bytes to msg after QDataStream wrote them is safe
    // because QDataStream is positioned at the end of what it wrote.
    QByteArray msg;
    {
        QDataStream ds(&msg, QIODevice::WriteOnly);
        ds.setByteOrder(QDataStream::BigEndian);
        const quint8 flags = kFlagPqEnabled | (asResponder ? kFlagIsResponder : 0);
        ds << kShakV2Magic << kShakV2Version << flags
           << static_cast<quint16>(idBytes.size());
    }
    msg.append(idBytes);
    msg.append(ourX25519Pub);
    // Append pq_payload_len then pq_payload as raw bytes.
    {
        const quint16 plen = static_cast<quint16>(pqPayload.size());
        msg.append(static_cast<char>((plen >> 8) & 0xFF));
        msg.append(static_cast<char>(plen & 0xFF));
    }
    msg.append(pqPayload);

    d->network->sendToPeer(peerId, msg);
    qCInfo(lcApp) << "Sent SHAK_v2 (UDP) to" << peerId.value
                  << "responder?" << asResponder
                  << "pq-payload" << pqPayload.size() << "bytes"
                  << "total" << msg.size() << "bytes"
                  << "(attempt" << attempt + 1 << "of" << kMaxShakAttempts << ")";

    // SHAK_v2 retry is driven by the legacy SHAK retry — we DON'T
    // schedule our own timer here. If the legacy SHAK retries 4 times,
    // each retry also sends a v2 (initiator side). Responder-side
    // sends are one-shot, not retried.
    Q_UNUSED(attempt);
#endif // SNAPCLONE_RUST_CRYPTO
}


void AppController::clearAddressCache()
{
    qCInfo(lcApp) << "Clearing address cache — next connections will re-resolve via DHT";
    if (d->storage) {
        d->storage->purgeDhtCache(0);  // age 0 = wipe everything
    }
    // Also clear any in-flight cache held in the connect helper if it has one
    // (currently AppController_connect uses storage as the single source of
    // truth, so DB wipe is sufficient).
}

} // namespace SnapClone::Core
