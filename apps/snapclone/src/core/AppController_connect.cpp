#include "AppController_p.hpp"
#include "network/AddressClassification.hpp"  // V32.32: isPrivate/isLoopback helpers
#include "core/VoiceWirePacket.hpp"            // V32.39+: voice-packet demux
#ifdef SNAPCLONE_RUST_CRYPTO
#include "crypto/RustCryptoService.hpp"       // V32.33+: PQ hybrid SHAK_v2
#endif
#include <QRegularExpression>
#include <QDateTime>
#include <QTimeZone>
#include <algorithm>  // V32.33+: std::fill for explicit zero-on-drop

namespace SnapClone::Core {

// ─── Signal wiring ───────────────────────────────────────────────────────────

void AppController::connectSubsystems()
{
    // ── Key exchange ──────────────────────────────────────────────────────
    // Wire format: magic(4) + idLen(2) + id(N) + pubKey(32)  ("SHAK" = 0x5348414B)
    // sendShak() is the implementation — see bottom of AppController.cpp.
    static constexpr quint32 kHandshakeMagic = 0x5348414B;
    // V32.33+ PQ Phase C: SHAK_v2 carries hybrid post-quantum KEM material.
    // Format: magic(4) + version(1) + flags(1) + idLen(2) + id(N) +
    //         x25519_pub(32) + pq_payload_len(2) + pq_payload(N).
    // flags bit 0 = PQ-enabled; bit 1 = is responder.
    // sendShakV2() is the implementation.
    static constexpr quint32 kShakV2Magic    = 0x53484B32; // "SHK2"
    static constexpr quint8  kShakV2Version  = 0x02;
    static constexpr quint8  kFlagPqEnabled  = 0x01;
    static constexpr quint8  kFlagIsResponder = 0x02;

    // Send SHAK to every newly TCP-connected peer.
    // sendShak handles retry with a cap of 4 attempts.
    connect(d->network.get(), &Network::NetworkManager::peerConnected,
            this, [this](const Core::UserId &peerId) {
                sendShak(peerId);
            });

    // ── Inbound payload dispatch ──────────────────────────────────────────
    connect(d->network.get(), &Network::NetworkManager::payloadReceived,
            this, [this](const UserId &sender,
                                        const QByteArray &payload) {
        if (payload.size() < 4) return;

        quint32 magic = 0;
        QDataStream peek(payload);
        peek.setByteOrder(QDataStream::BigEndian);
        peek >> magic;

        if (magic == kHandshakeMagic) {
            // Parse: magic(4) + idLen(2) + id(N) + pubKey(32)
            constexpr int kHeaderSize = 6; // 4 (magic) + 2 (idLen)
            if (payload.size() < kHeaderSize + 1 + 32) return;

            quint16 idLen = 0;
            QDataStream ds(payload);
            ds.setByteOrder(QDataStream::BigEndian);
            ds >> magic >> idLen;

            // Cap idLen: a valid userId is "user@192.168.x.x" — well under 256 bytes.
            // A 65535-byte identity would be a DoS / injection attempt.
            constexpr quint16 kMaxIdLen = 256;
            if (idLen == 0 || idLen > kMaxIdLen
                || payload.size() < kHeaderSize + idLen + 32) return;

            const QString realId =
                QString::fromUtf8(payload.mid(kHeaderSize, idLen));
            const QByteArray peerPubKey = payload.mid(kHeaderSize + idLen, 32);
            if (realId.isEmpty() || peerPubKey.size() != 32) return;

            // Extract the bare sc-ID from the display string if present.
            // realId is "user@ip (sc-xxxx)" — the sc-ID is the stable key used
            // everywhere (connectByDhtId, snap recipientId, session lookup).
            // Using the full display string as the peer ID causes:
            //   • saveContact stores a row nobody finds with loadContact("sc-xxxx")
            //   • encryptFor("sc-xxxx") misses the session key stored under the full string
            //   • weInitiated check misses relookupCount["sc-xxxx"]
            // Fix: always normalise to the bare sc-ID.
            static const QRegularExpression kScRx(
                QStringLiteral(R"(\(sc-[0-9a-f]{8}\))"),
                QRegularExpression::CaseInsensitiveOption);
            const auto scMatch = kScRx.match(realId);
            const QString stableId = scMatch.hasMatch()
                ? scMatch.captured(0).mid(1, scMatch.captured(0).size() - 2) // "sc-xxxx"
                : realId; // fallback: use realId as-is if no sc-ID suffix

            const Core::UserId realPeerId{ stableId };

            // V32.33: TOFU on pubkey changes (CRYPTO_REVIEW.md S-20).
            //
            // Previously, this code path blindly called savePeerPublicKey,
            // which uses ON CONFLICT DO UPDATE — silently replacing any
            // existing stored key. That made persistent-MITM trivial: an
            // attacker sends a SHAK with their own pubkey claiming to be
            // an established contact, our DB silently replaces the real
            // contact's pubkey with the attacker's, and from that moment
            // we encrypt-to-attacker thinking it's the contact.
            //
            // New behaviour:
            //   • If no key is stored yet — Trust On First Use, save it.
            //     This is the "first contact" path. (Defense against
            //     first-contact-only attackers requires signed SHAK, see
            //     S-2 — coming in the next change.)
            //   • If the stored key matches — fine, no-op.
            //   • If the stored key differs — DO NOT replace. Record the
            //     attempted change in pending_key_changes and continue
            //     using the existing key. The user reviews + accepts or
            //     rejects via Settings.
            //
            // While a pending change is outstanding for this peer, we
            // continue encrypting/decrypting with the OLD key. That
            // means: if real Alice actually did re-install, her snaps
            // will MAC-fail against the old key, she'll appear silent,
            // and the user will discover the pending review through the
            // resulting "where are Alice's snaps?" UX. Acceptable for v1.
            const QByteArray storedKey = d->storage->loadPeerPublicKey(realPeerId);
            bool acceptKey = false;
            if (storedKey.isEmpty()) {
                // First contact for this peer — save and proceed.
                acceptKey = true;
            } else if (storedKey == peerPubKey) {
                // Same key as before — proceed without resaving.
                acceptKey = true;
            } else {
                // KEY MISMATCH. Record for user review; do NOT replace.
                qCWarning(lcApp) << "PUBKEY CHANGED for" << realPeerId.value
                                    << "— recorded as pending; existing key kept."
                                    << " (See Settings → Pending key changes.)";
                d->storage->recordPendingKeyChange(realPeerId, peerPubKey, storedKey);
                // Continue with the OLD key — register it back so all the
                // alias-registrations below use the trusted key, not the
                // attacker-supplied one.
                d->crypto->registerPeerPublicKey(realPeerId, storedKey);
                // Don't touch storage; storedKey is already there.
            }

            if (acceptKey) {
                d->crypto->registerPeerPublicKey(realPeerId, peerPubKey);
                d->storage->savePeerPublicKey(realPeerId, peerPubKey);
            }

            // Also register under the full display string and transport ID
            // so any code using those as keys also finds the session key.
            // V32.33: use the TRUSTED key (storedKey if we kept it, else the
            // new one we just accepted) so a rejected pubkey never leaks
            // into the alias registrations.
            const QByteArray trustedKey = acceptKey ? peerPubKey : storedKey;
            if (stableId != realId) {
                const Core::UserId fullId{ realId };
                d->crypto->registerPeerPublicKey(fullId, trustedKey);
            }
            d->crypto->registerPeerPublicKey(sender, trustedKey);

            if (sender.value != realId) {
                // Inbound transport (e.g. "inbound:ip:port") — promote to real ID.
                // Record the transport→real mapping so inbound snaps show
                // the peer's real identity rather than "inbound:ip:port".
                d->transportToRealId[sender.value] = realId;

                // Move the live inbound transport into the real-ID session so
                // sendToPeer(realId) can deliver snaps even if the outbound
                // connection (to the peer's port 9876) is blocked or timing out.
                // This is the key fix for one-sided firewall configurations where
                // B can connect to A but A cannot connect to B.
                d->network->promoteInboundSession(realPeerId,
                                                   Core::UserId{ sender.value });
            }
            // Note: when sender.value == realId (relay transport where the peer's
            // sc-ID is used as the transport key), no promotion is needed — the
            // session is already keyed under the real ID.

            qCInfo(lcApp) << "Session key registered for" << realId
                          << "(transport:" << sender.value << ")";
            // Exchange peer tables over any newly established session.
            if (d->directDiscovery)
                d->directDiscovery->onPeerConnected(
                    realPeerId, d->network.get());

            // Persist the peer as a contact so they survive restarts.
            // Use the full display name from the SHAK payload as displayName.
            // Nickname is left empty so the user can set their own later.
            // Build the contact with the display name stripped from realId.
            // "kesslerd@192.168.2.178 (sc-32a51661)" → "kesslerd"
            // We save this immediately so the contact persists even if the
            // connection is dropped before the Known-promotion save below.
            Core::Contact peerContact;
            peerContact.id    = realPeerId;
            peerContact.state = Core::ContactState::Known; // save as Known immediately
            peerContact.online = true;
            // Round lastSeen to 5-min bucket — prevents presence pattern fingerprinting
            { const qint64 ms = QDateTime::currentMSecsSinceEpoch();
              peerContact.lastSeen = QDateTime::fromMSecsSinceEpoch(
                  (ms / 300000LL) * 300000LL, QTimeZone::utc()); }
            {
                QString dn = realId;
                const qsizetype paren = dn.indexOf(QLatin1String(" ("));
                if (paren > 0) dn = dn.left(paren); // strip " (sc-xxxx)"
                const qsizetype at = dn.indexOf(QLatin1Char('@'));
                if (at > 0) dn = dn.left(at);        // strip "@ip"
                peerContact.displayName = dn.isEmpty() ? stableId : dn;
            }
            d->storage->saveContact(peerContact);

            // Mutual connection: when we receive an inbound SHAK from a
            // sc- peer, proactively connect back to them outbound as well.
            // This gives BOTH sides an independent socket to the other,
            // so either side can survive the other's server going down.
            // Skip if we already have an outbound session to this peer.
            if (stableId.startsWith(QStringLiteral("sc-"))
                && !d->network->hasAnyLiveSession(realPeerId)) {
                // Use stableId ("sc-xxxx"), NOT realId ("user@ip (sc-xxxx)").
                // connectByDhtId expects the bare DHT routing ID format.
                qCInfo(lcApp) << "Mutual connect to" << stableId << "via DHT";
                connectByDhtId(stableId);
            }


            // Respond with our key — but only ONCE per transport ID.
            // Without this guard each received handshake triggers another send,
            // which the peer receives and sends back, creating an infinite loop
            // that eventually overflows the call stack (segfault).
            if (!d->handshakeRespondedTo.contains(sender.value)) {
                d->handshakeRespondedTo.insert(sender.value);
                sendShak(sender); // respond with our identity/key
            }

            // Now that we know the real ID, tell the UI so the friend list
            // shows "user@192.168.2.214" instead of "inbound:192.168.2.214:40794".
            // We emit peerConnected(realId) here; the NetworkManager's peerConnected
            // signal for the inbound:... ID is suppressed in the handler below.
            // Run full peer-connect logic for ALL SHAK completions —
            // relay connections arrive with sender.value == realId (no inbound promotion
            // needed) but still need approval checks, peerConnected, etc.
            {
                // A stranger is someone we did NOT initiate contact with
                // AND who is not already a Known contact in the DB.
                // We catch both direct inbound (transport "inbound:ip:port") AND
                // relay-based inbound (transport "relay:..." or the peer's sc-id
                // used as transport when they dialled through a relay first).
                // weInitiated: did WE ask to connect to this peer?
                // relookupCount and pendingAddresses are keyed by sc-ID (connectByDhtId).
                // Check both the stable sc-ID and the full realId for robustness.
                const bool weInitiated = d->relookupCount.contains(stableId)
                                      || d->relookupCount.contains(realId)
                                      || d->pendingAddresses.contains(stableId)
                                      || d->pendingAddresses.contains(realId);

                // Use loadContact for a single-row lookup instead of scanning all contacts.
                const auto existingContact = d->storage->loadContact(realPeerId);
                const bool isKnown = existingContact.has_value()
                    && existingContact->state == Core::ContactState::Known;

                const bool isInboundStranger = !weInitiated
                    && !isKnown
                    && !d->deniedPeers.contains(stableId); // use stable sc-ID, not display string

                if (isInboundStranger && !isKnown) {
                    // Gate: ask the user to approve or deny this connection.
                    if (!d->pendingInbound.contains(realId)) {
                        d->pendingInbound[realId] = peerContact.displayName;
                        qCInfo(lcApp) << "[INBOUND] Awaiting approval for new peer:"
                                      << realId;
                        emit inboundConnectionRequest(realPeerId,
                                                      peerContact.displayName);
                    }
                    // Don't emit peerConnected yet — wait for approveInboundPeer().
                    return;
                }

                // Promote to Known — this peer has successfully connected.
                // Must happen BEFORE emitting peerConnected so the UI card
                // shows in the right section immediately.
                {
                    Core::Contact known;
                    known.id    = realPeerId;
                    known.state = Core::ContactState::Known;
                    // Load existing data so we don't blank nickname/notes
                    const auto existing = d->storage->loadContact(realPeerId);
                    if (existing.has_value()) {
                        known = *existing;
                        known.state  = Core::ContactState::Known;
                        known.online = true;
                    } else {
                        known.displayName = peerContact.displayName;
                        known.online      = true;
                    }
                    { const qint64 ms = QDateTime::currentMSecsSinceEpoch();
                known.lastSeen = QDateTime::fromMSecsSinceEpoch(
                    (ms / 300000LL) * 300000LL, QTimeZone::utc()); }
                    d->storage->saveContact(known);
                }
                emit peerConnected(realPeerId);
                // V32.27: record that we just completed a full session
                // with this peer. last_connected_at feeds future
                // staleness-based eviction policies and is distinct from
                // last_seen_at (which fires on any presence signal,
                // including bare gossip beacons).
                if (d->storage) d->storage->touchLastConnected(realPeerId);
                emit peerConnectionStatus(realPeerId, QString(), QString()); // clear status
                // Emit WS transport stat — read lastAttemptedAddr before clearing it.
                {
                    const Core::PeerAddress wsAddr =
                        d->lastAttemptedAddr.value(realPeerId.value);
                    if (!wsAddr.host.isEmpty()) {
                        // V32.32: helper replaces 3-line inline RFC1918 check.
                        const bool isLan =
                            SnapClone::Network::AddressClass::isPrivate(wsAddr.host);
                        const bool isRelay = wsAddr.host.contains(QStringLiteral("transit")) ||
                                             wsAddr.port == 4001;
                        const QString quality = isRelay ? tr("Relay") :
                                                isLan   ? tr("LAN") :
                                                          tr("Internet");
                        emit peerStatUpdate(realPeerId, quality);
                    } else {
                        emit peerStatUpdate(realPeerId, tr("Connected"));
                    }
                }
                // Reset relookup counter — peer is now connected, so future
                // disconnects get the full kMaxRelookups attempts again.
                d->relookupCount.remove(realPeerId.value);
                d->mailboxPublished.remove(realPeerId.value);
                d->pendingAddresses.remove(realPeerId.value);
                d->activeRelays.remove(realPeerId.value); // clear relay guard on successful connect
                d->lastAttemptedAddr.remove(realPeerId.value);

                // Fix 2: If the handshake arrived on an inbound transport, extract
                // the peer's host and save it under their WS listen port so
                // DirectDiscovery can probe it on the next startup (LAN reconnect).
                // For internet peers this is a best-effort hint; DHT handles the rest.
                if (sender.value.startsWith(QStringLiteral("inbound:"))) {
                    const QStringList parts = sender.value.split(QLatin1Char(':'));
                    // "inbound" : host : ephemeral_port  (3 or 4 parts for IPv6)
                    if (parts.size() >= 3) {
                        // Reconstruct host robustly: everything between "inbound:" and the
                        // last colon-separated port number. For IPv4 "inbound:1.2.3.4:port"
                        // this gives "1.2.3.4". For IPv6 "inbound:fe80::1%eth0:port"
                        // this reassembles the full IPv6 literal.
                        const QString withoutPrefix = sender.value.mid(QString("inbound:").size());
                        const auto lastColon = withoutPrefix.lastIndexOf(QLatin1Char(':'));
                        const QString inboundHost = (lastColon > 0)
                            ? withoutPrefix.left(lastColon)
                            : withoutPrefix;
                        // Store at the standard WS listen port, not the ephemeral one.
                        // This is a hint for LAN probing; DHT gives a verified addr.
                        constexpr quint16 kDefaultWsPort = 9876;
                        const Core::PeerAddress hintAddr{ inboundHost, kDefaultWsPort };
                        if (!inboundHost.isEmpty() && !inboundHost.startsWith(QStringLiteral("10."))
                            && !inboundHost.startsWith(QStringLiteral("127."))) {
                            // Only save non-loopback addresses; LAN (192.168/172.x) are fine.
                            d->storage->savePeerAddress(realPeerId, hintAddr);
                            qCInfo(lcApp) << "Saved inbound hint addr for" << realPeerId.value
                                          << "@" << hintAddr.toString();
                        } else if (!inboundHost.isEmpty()) {
                            d->storage->savePeerAddress(realPeerId, hintAddr);
                            qCInfo(lcApp) << "Saved LAN hint addr for" << realPeerId.value
                                          << "@" << hintAddr.toString();
                        }
                    }
                }
                // Race-then-settle: after 60s of stable connection,
                // consolidate any redundant sessions for this peer.
                {
                    const QString spid = realPeerId.value;
                    if (!d->stabilityTimers.contains(spid)) {
                        auto *t = new QTimer(this);
                        t->setSingleShot(true);
                        connect(t, &QTimer::timeout, this, [this, spid, t]() {
                            d->stabilityTimers.remove(spid);
                            qCInfo(lcApp) << "Peer" << spid
                                          << "stable for 60s — consolidating sessions.";
                            d->network->consolidateSessions(Core::UserId{spid});
                            // deleteLater LAST: Qt holds it until the next event
                            // loop tick anyway, but moving it to the end avoids
                            // any future maintainer accidentally adding code that
                            // dereferences `t` after the delete-queue point.
                            t->deleteLater();
                        });
                        d->stabilityTimers[spid] = t;
                        t->start(60000);
                    }
                }
                // Mark peer as v3 capable before triggering sync
                // so onPeerConnected sends v3 SYNC (not v2 SyncReq).
                if (d->router) d->router->setPeerVersion(realPeerId, 3);
                // Trigger reconnect sync — router sends v3 SYNC to discover
                // any snaps this peer missed while disconnected.
                if (d->router) d->router->onPeerConnected(realPeerId);
            }

        } else if (magic == kShakV2Magic) {
#ifndef SNAPCLONE_RUST_CRYPTO
            // V32.33+ PQ: SHAK_v2 requires the Rust crypto backend for
            // ML-KEM-768 ops. Builds with SNAPCLONE_RUST_CRYPTO=OFF
            // ignore the packet — the peer's v1 SHAK (which is sent
            // alongside v2 during transition) handles connectivity.
            qCDebug(lcApp) << "SHK2 received but Rust crypto disabled; dropping";
            return;
#else
            // V32.33+ PQ Phase C: SHAK_v2 with hybrid KEM material.
            //
            // Wire format (big-endian):
            //   magic(4) + version(1) + flags(1) + idLen(2) + id(N) +
            //   x25519_pub(32) + pq_payload_len(2) + pq_payload(N).
            //
            // For Phase C we do NOT enforce a signature — that's Phase D.
            // A peer that sends us a v2 SHAK with a forged identity gets
            // through this layer as far as v1 would have. The added value
            // of v2 is the PQ session key — IF the peer is real, we get
            // PQ-FS for free; if the peer is an attacker, they end up
            // with a different session key than the legitimate peer
            // would have derived, and subsequent AEAD-tagged payloads
            // fail GCM verification.
            constexpr int kV2HeaderFixed = 4 + 1 + 1 + 2; // magic+version+flags+idLen
            if (payload.size() < kV2HeaderFixed) return;

            QDataStream ds(payload);
            ds.setByteOrder(QDataStream::BigEndian);
            quint32 m = 0; quint8 ver = 0, flags = 0; quint16 idLen = 0;
            ds >> m >> ver >> flags >> idLen;

            if (ver != kShakV2Version) {
                qCWarning(lcApp) << "SHK2: unknown version" << ver << "— dropping";
                return;
            }
            if ((flags & kFlagPqEnabled) == 0) {
                qCWarning(lcApp) << "SHK2: PQ flag clear — dropping";
                return;
            }
            constexpr quint16 kMaxIdLen = 256;
            if (idLen == 0 || idLen > kMaxIdLen) return;

            // Compute offsets defensively.
            const int offId          = kV2HeaderFixed;
            const int offX25519Pub   = offId + idLen;
            const int offPqLen       = offX25519Pub + 32;
            if (payload.size() < offPqLen + 2) return;

            const QString realIdStr = QString::fromUtf8(
                payload.mid(offId, idLen));
            const QByteArray peerX25519Pub = payload.mid(offX25519Pub, 32);

            // Read pq_payload_len.
            const quint16 pqLen = (static_cast<quint8>(payload[offPqLen]) << 8)
                                | static_cast<quint8>(payload[offPqLen + 1]);
            const int offPq = offPqLen + 2;
            if (pqLen == 0 || pqLen > 4096
                || payload.size() < offPq + pqLen) {
                qCWarning(lcApp) << "SHK2: invalid pq payload length" << pqLen;
                return;
            }
            const QByteArray pqPayload = payload.mid(offPq, pqLen);

            // Normalize sc-ID like the v1 path does.
            static const QRegularExpression kScRx(
                QStringLiteral(R"(\(sc-[0-9a-f]{8}\))"),
                QRegularExpression::CaseInsensitiveOption);
            const auto scMatch = kScRx.match(realIdStr);
            const QString stableId = scMatch.hasMatch()
                ? scMatch.captured(0).mid(1, scMatch.captured(0).size() - 2)
                : realIdStr;
            const Core::UserId realPeerId{ stableId };
            const bool isResponder = (flags & kFlagIsResponder) != 0;

            qCInfo(lcApp) << "SHK2 received from" << realPeerId.value
                          << "responder?" << isResponder
                          << "pq-payload" << pqLen << "bytes";

            auto *rustCrypto = dynamic_cast<Crypto::RustCryptoService *>(
                d->crypto.get());
            if (!rustCrypto || !rustCrypto->pqAvailable()) {
                qCWarning(lcApp) << "SHK2: no PQ-capable crypto backend; dropping";
                return;
            }

            if (!isResponder) {
                // The peer is initiating PQ with us. pqPayload is their
                // ML-KEM ek. We encapsulate, derive a hybrid session
                // key, and send our SHAK_v2 (as responder) back with
                // the resulting ct.
                if (pqLen != static_cast<int>(sc_mlkem_ek_len())) {
                    qCWarning(lcApp) << "SHK2 initiator: wrong ek size"
                                     << pqLen << "expected" << sc_mlkem_ek_len();
                    return;
                }
                // V32.48: GLARE RESOLUTION. If we ALSO have an outstanding
                // initiation to this peer, both sides initiated at once. The
                // duplicate guards below only catch the same handshake
                // redelivered — they do NOT catch two DISTINCT handshakes. Left
                // alone, glare runs both: each side responds to the other's ek
                // (storing one key) and then completes its OWN initiation when
                // the peer's responder ct arrives (overwriting with another).
                // Because each side's completion uses ITS handshake's secret,
                // A ends up keyed to the A-initiated secret and B to the
                // B-initiated one — they differ, so every subsequent decrypt
                // fails the MAC. Resolve deterministically: the peer with the
                // smaller stableId is the canonical initiator; that one
                // handshake is what both sides keep.
                if (d->pendingPqEphemerals.contains(realPeerId.value)) {
                    if (d->stableId < realPeerId.value) {
                        // We're the canonical initiator — keep ours, ignore
                        // theirs. They (larger id) will respond to ours.
                        qCInfo(lcApp) << "SHK2 glare with" << realPeerId.value
                                      << "— keeping our initiation (smaller id),"
                                      << "ignoring peer's";
                        return;
                    }
                    // We're the canonical responder (larger id): respond to
                    // their initiation below, but ABANDON our own so a late
                    // responder ct for it can't overwrite the agreed key.
                    qCInfo(lcApp) << "SHK2 glare with" << realPeerId.value
                                  << "— abandoning our initiation (larger id),"
                                  << "responding to peer's";
                    auto pit = d->pendingPqEphemerals.find(realPeerId.value);
                    if (pit != d->pendingPqEphemerals.end()) {
                        QByteArray &dk = pit.value().decapsulationKey;
                        std::fill(dk.begin(), dk.end(), '\0');
                        d->pendingPqEphemerals.erase(pit);
                    }
                }
                // V32.34+ FIX: only run the responder logic ONCE per peer
                // per session. The guard below previously inserted into
                // d->handshakeRespondedTo but called sendShakV2
                // UNCONDITIONALLY outside the if — so the guard was dead
                // code. Duplicate `responder=false` SHK2s (the same blob
                // redelivered across UDP punch + relay + gossip) each
                // re-ran respondHybridHandshake, which ENCAPSULATES FRESH
                // every call → a new ct + new session key each time, and
                // each overwrote d->pqSessionKeys[peer] (see
                // sendShakV2 responder path). The peer finished from the
                // FIRST ct, so once our stored key rotated to a later one,
                // every subsequent decrypt hit "MAC verification failed".
                // That's the decrypt-failure storm in the field logs. By
                // gating the resend, the first response wins and the
                // session key stays stable; later duplicates are ignored.
                if (d->handshakeRespondedTo.contains(realPeerId.value)) {
                    qCDebug(lcApp) << "SHK2 initiator: already responded to"
                                   << realPeerId.value
                                   << "this session — ignoring duplicate";
                    return;
                }
                d->handshakeRespondedTo.insert(realPeerId.value);
                sendShakV2(realPeerId, /*asResponder=*/true,
                           /*peerMlkemEk=*/pqPayload,
                           /*peerX25519Pub=*/peerX25519Pub);
            } else {
                // The peer is responding to OUR earlier v2 SHAK with the
                // ML-KEM ciphertext. Look up the stashed ephemeral dk
                // and decapsulate to derive the hybrid session key.
                if (pqLen != static_cast<int>(sc_mlkem_ct_len())) {
                    qCWarning(lcApp) << "SHK2 responder: wrong ct size"
                                     << pqLen << "expected" << sc_mlkem_ct_len();
                    return;
                }
                const auto it = d->pendingPqEphemerals.find(realPeerId.value);
                if (it == d->pendingPqEphemerals.end()) {
                    // V32.34+: distinguish two cases. If we already derived
                    // a hybrid session key for this peer, this is simply a
                    // DUPLICATE of the responder ct we already consumed (the
                    // same SHK2 redelivered across UDP punch + relay +
                    // gossip). The first copy completed the handshake and
                    // erased the ephemeral; the duplicate finding no pending
                    // ephemeral is EXPECTED, not a failure — log at debug so
                    // it stops masquerading as an error in the field logs.
                    // Only warn if we have NO session key either, which is a
                    // genuine out-of-order / lost-ephemeral situation.
                    if (d->pqSessionKeys.contains(realPeerId.value)) {
                        qCDebug(lcApp) << "SHK2 responder: duplicate ct for"
                                       << realPeerId.value
                                       << "— handshake already completed, ignoring";
                    } else {
                        qCWarning(lcApp) << "SHK2 responder: no pending ephemeral for"
                                         << realPeerId.value << "— dropped";
                    }
                    return;
                }
                const QByteArray sessionKey = rustCrypto->finishHybridHandshake(
                    Core::UserId{ d->stableId },
                    peerX25519Pub,
                    it.value().decapsulationKey,
                    pqPayload);
                // ZERO and remove the ephemeral dk immediately on success
                // — PQ-FS depends on the dk being unrecoverable once
                // we're past this point. QByteArray doesn't zero
                // automatically on destruction, so we overwrite first.
                {
                    QByteArray &dk = it.value().decapsulationKey;
                    std::fill(dk.begin(), dk.end(), '\0');
                }
                d->pendingPqEphemerals.erase(it);

                if (sessionKey.isEmpty()) {
                    qCWarning(lcApp) << "SHK2 responder: finishHybridHandshake failed";
                    return;
                }
                d->pqSessionKeys.insert(realPeerId.value, sessionKey);
                qCInfo(lcApp) << "SHK2 responder: derived hybrid session key for"
                              << realPeerId.value << "(" << sessionKey.size()
                              << "bytes)";
            }
#endif // SNAPCLONE_RUST_CRYPTO

        } else if (Core::VoiceWire::isVoicePacket(payload)) {
            // V32.39+: group-voice frame / floor-control. Resolve to the stable
            // sender id (same transport→realId rewrite as snaps) and surface it;
            // the open conversation's voice route consumes it, else it's dropped.
            // V32.47: any inbound voice/floor frame means a call is live — note
            // it so the background reconnection loop backs off.
            noteVoiceActivity();
            const auto it = d->transportToRealId.find(sender.value);
            const Core::UserId effectiveSender =
                (it != d->transportToRealId.end())
                    ? Core::UserId{ *it }
                    : sender;
            emit voicePayloadReceived(effectiveSender, payload);

        } else {
            // Regular snap payload.  Rewrite senderId if we have a real ID
            // for this transport connection — so the inbox shows the human-
            // readable identity rather than "inbound:192.168.2.x:port".
            const auto it = d->transportToRealId.find(sender.value);
            const Core::UserId effectiveSender =
                (it != d->transportToRealId.end())
                    ? Core::UserId{ *it }
                    : sender;
            d->router->routeInbound(payload, effectiveSender);
        }
    });

    // Router → AppController → UI
    // Use a lambda so we can emit peerLocationReceived before snapReceived.
    connect(d->router.get(), &SnapRouter::inboundReady,
            this, [this](const Core::Snap &snap) {
                if (snap.hasLocation()) {
                    Core::PeerLocation loc;
                    loc.peerId     = snap.senderId.value;
                    loc.lat        = snap.locLat;
                    loc.lon        = snap.locLon;
                    loc.accuracyM  = snap.locAccM;
                    loc.recordedAt = snap.sentAt;
                    loc.source     = QStringLiteral("snap");
                    emit peerLocationReceived(snap.senderId, loc);
                }
                emit snapReceived(snap);
            });
    connect(d->router.get(), &SnapRouter::outboundDelivered,
            this, &AppController::snapDelivered);
    connect(d->router.get(), &SnapRouter::routingFailed,
            this, [this](const SnapId &, const QString &reason) {
                emit errorOccurred(
                    QStringLiteral("Routing failed: %1").arg(reason));
            });
    // Offline snap: snap saved to outbox, peer unreachable. Trigger reconnect
    // so SYNC delivers it as soon as the peer comes online. Emit snapQueued
    // so UI can show a pending indicator instead of an error.
    connect(d->router.get(), &SnapRouter::snapQueuedForOfflinePeer,
            this, [this](const SnapId &id, const UserId &recipientId) {
                emit snapQueued(id, recipientId);
                // Kick off a DHT reconnect — if peer comes online quickly
                // we'll sync the snap without the user doing anything.
                if (recipientId.value.startsWith(QStringLiteral("sc-")))
                    connectByDhtId(recipientId.value);
            });

    // Network peer events → AppController → UI
    // peerConnected: suppress duplicate events for the same physical peer.
    // When two devices discover each other via gossip, both create outbound
    // connections, so peerConnected fires for BOTH the gossip ID AND the
    // "inbound:ip:port" transport ID once the handshake reveals the real ID.
    // We deduplicate by tracking which real IDs we have already announced.
    connect(d->network.get(), &Network::NetworkManager::peerConnected,
            this, [this](const Core::UserId &id) {
                // Never forward raw "inbound:ip:port" IDs to the UI.
                if (id.value.startsWith(QLatin1String("inbound:"))) return;
                // Promote to Known if this is a direct outbound connection
                // (gossip LAN, DHT direct — no SHAK handler ran for these).
                if (id.value.startsWith(QLatin1String("sc-"))) {
                    if (const auto existing = d->storage->loadContact(id)) {
                        // Existing contact — promote to Known and mark online.
                        Core::Contact known = *existing;
                        known.state    = Core::ContactState::Known;
                        known.online   = true;
                        { const qint64 ms = QDateTime::currentMSecsSinceEpoch();
                known.lastSeen = QDateTime::fromMSecsSinceEpoch(
                    (ms / 300000LL) * 300000LL, QTimeZone::utc()); }
                        d->storage->saveContact(known);
                    } else {
                        // First time connecting outbound to this peer —
                        // create a minimal Known contact so the UI has a row.
                        // The displayName will be updated when the SHAK arrives
                        // and gives us their real identity string.
                        Core::Contact fresh;
                        fresh.id          = id;
                        fresh.displayName = id.value; // sc-XXXXXXXX as placeholder
                        fresh.state       = Core::ContactState::Known;
                        fresh.online      = true;
                        { const qint64 ms = QDateTime::currentMSecsSinceEpoch();
                fresh.lastSeen = QDateTime::fromMSecsSinceEpoch(
                    (ms / 300000LL) * 300000LL, QTimeZone::utc()); }
                        d->storage->saveContact(fresh);
                        qCInfo(lcApp) << "[CONTACT] Created placeholder for new outbound peer:"
                                      << id.value;
                    }
                }
                emit peerConnected(id);
            });
    // peerDisconnected: translate transport ID to real ID before forwarding,
    // then clean up the mapping entry so the map doesn't grow unboundedly.
    // Each new inbound connection gets a unique "inbound:ip:port" key; without
    // cleanup these accumulate for the lifetime of the process.
    connect(d->network.get(), &Network::NetworkManager::peerDisconnected,
            this, [this](const Core::UserId &id) {
                const auto it = d->transportToRealId.find(id.value);
                const Core::UserId effective =
                    (it != d->transportToRealId.end())
                        ? Core::UserId{ *it } : id;
                // Remove the mapping now that the transport is gone.
                if (it != d->transportToRealId.end())
                    d->transportToRealId.erase(it);
                // Also clean up the handshake guard for this transport.
                // V32.34+: clear under BOTH the transport id AND the
                // resolved real/stable id. The v1 SHK guard keys on the
                // transport id (sender.value), but the v2 SHK2 responder
                // guard keys on the normalized stable id (realPeerId, e.g.
                // "sc-051d7c06"), which can differ from the transport id.
                // Clearing only the transport id would leave the v2 entry
                // stranded → a genuine reconnect would be silently blocked
                // from re-responding to the peer's PQ handshake forever.
                // Clear both so reconnect always re-handshakes cleanly.
                d->handshakeRespondedTo.remove(id.value);
                if (effective.value != id.value)
                    d->handshakeRespondedTo.remove(effective.value);
                if (auto *t = d->stabilityTimers.take(effective.value)) {
                    t->stop();
                    t->deleteLater();
                    qCInfo(lcApp) << "Peer" << effective.value
                                  << "disconnected — stability timer cancelled, will re-race.";
                }
                // V32.34+: also clean up per-peer state that previously
                // accumulated across the session lifetime. These maps
                // are keyed by sc-ID; removing them on disconnect lets
                // a new session for the same peer start fresh, AND
                // prevents long-running session bloat from peers we
                // contacted once and never again (DHT random walks,
                // mailbox encounters). Some maps (lastAttemptedAddr)
                // are intentionally NOT cleared here because reconnect
                // logic reads them to mark the previous attempt failed.
                d->mailboxPublished.remove(effective.value);
                d->relookupCount.remove(effective.value);
                d->pendingDhtLookups.remove(effective.value);
                if (d->activeLookupHandles.contains(effective.value)) {
                    QObject::disconnect(d->activeLookupHandles.take(effective.value));
                }
                d->lookupGeneration.remove(effective.value);
                // V32.34+: clear PQ ephemeral state for this peer. The
                // ephemeral kex private key we generated when sending a
                // SHAK is useless once the session is closed; keeping
                // it just leaks key material.
                //
                // NOTE on pqSessionKeys: deliberately NOT cleared here.
                // Design intent (from the V32.27 PQ rollout) is "derive
                // once per peer, reuse across reconnects" — that's the
                // perf-vs-forward-secrecy trade-off taken at design
                // time. Clearing here would force a new handshake on
                // every reconnect (a meaningful UX regression). The
                // unbounded growth is mitigated by a session-cap
                // applied at insert time (filed for future); for now
                // the leak is bounded by total-peers-ever-met which
                // for normal usage is small.
                d->pendingPqEphemerals.remove(effective.value);
                // Clear per-peer routing state so next session starts fresh.
                if (d->router) d->router->clearPeerState(effective);
                emit peerDisconnected(effective);

                // Fix 3: if this was a known sc- contact that connected inbound-only
                // (we never initiated outbound), the relookup mechanism won't fire
                // automatically. Schedule a reconnect attempt so we stay connected
                // to contacts that reach us rather than waiting for them to retry.
                if (effective.value.startsWith(QStringLiteral("sc-"))
                    && !d->network->hasAnyLiveSession(effective)) {
                    const QString reconnId = effective.value;

                    // V32.28: context-adaptive reconnect delay. Default
                    // 10s gives the peer time to come back if they had a
                    // brief flap. But when `userJustActed(2000)` the user
                    // is actively trying to send/view — push reconnect
                    // immediately (1s) so they don't wait. The user-active
                    // signal includes shutter press, snap-open, send tap.
                    int reconnDelayMs = 10000;
                    if (d->appContext) {
                        const auto &ctx = d->appContext->context();
                        if (ctx.userJustActed(2000)) {
                            reconnDelayMs = 1000;
                            qCInfo(lcApp) << "[RECONNECT] User just acted —"
                                          << "shortening reconnect delay to 1s for"
                                          << reconnId;
                        }
                    }

                    QTimer::singleShot(reconnDelayMs, this, [this, reconnId]() {
                        const Core::UserId rid{ reconnId };
                        if (d->network->hasAnyLiveSession(rid)) return; // reconnected already
                        if (d->deniedPeers.contains(reconnId)) return;  // user denied this peer
                        // Only retry if this peer is actually a known contact
                        const auto existing = d->storage->loadContact(
                            Core::UserId{ reconnId });
                        const bool isKnown = existing.has_value()
                            && existing->state == Core::ContactState::Known;
                        if (!isKnown) return;
                        qCInfo(lcApp) << "[RECONNECT] Known contact" << reconnId
                                      << "disconnected — scheduling DHT reconnect";
                        connectByDhtId(reconnId);
                    });
                }
            });
    connect(d->network.get(), &Network::NetworkManager::networkError,
            this, &AppController::errorOccurred);

    // When a peer disconnects with no known outbound address (e.g. their IP
    // changed), try to find them again via DHT before giving up.
    connect(d->network.get(), &Network::NetworkManager::peerNeedsRelookup,
            this, [this](const Core::UserId &id) {
                if (!id.value.startsWith(QStringLiteral("sc-"))) return;
                // If the peer already has a live inbound or outbound session
                // (e.g. they connected to us while our outbound was failing),
                // don't clear addresses or schedule a new lookup — the session
                // is working and the lookup would only produce more noise.
                if (d->network->hasAnyLiveSession(id)) {
                    qCInfo(lcApp) << "Peer" << id.value
                                  << "already has a live session (in or out) — skipping relookup.";
                    return;
                }
                const QString idCopy = id.value;

                // If UDP channel is already active, no need for relookup —
                // data can flow over UDP even without TCP.
                if (d->udpChannel && d->udpChannel->isConnected(id)) {
                    qCInfo(lcApp) << "Peer" << id.value
                                  << "reachable via UDP channel — skipping TCP relookup.";
                    return;
                }

                // Re-punch on every TCP failure — refreshes CGNAT mapping.
                // Both sides time out simultaneously → synchronized re-punch.
                if (d->nat && d->peerPunchAddr.contains(idCopy)) {
                    const Core::PeerAddress punchAddr = d->peerPunchAddr[idCopy];
                    const Core::UserId pid{ idCopy };
                    qCInfo(lcApp) << "[REPUNCH] Re-punching" << idCopy
                                  << "@" << punchAddr.toString();
                    d->nat->initiateHolePunch(pid, punchAddr);
                    if (d->udpChannel) d->udpChannel->connectToPeer(pid, punchAddr);
                    // Schedule another re-punch in 10s if still disconnected
                    QTimer::singleShot(10000, this, [this, idCopy, punchAddr]() {
                        if (!d->nat || !d->udpChannel) return;
                        const Core::UserId pid2{ idCopy };
                        if (!d->network->hasAnyLiveSession(pid2) &&
                            !d->udpChannel->isConnected(pid2)) {
                            qCInfo(lcApp) << "[REPUNCH] Scheduled re-punch to"
                                          << idCopy;
                            d->nat->initiateHolePunch(pid2, punchAddr);
                        }
                    });
                }

                // If there are still addresses in the queue, try the next one
                // WITHOUT a full DHT relookup. This cycles through all candidate
                // addresses (LAN first, then public) with 3s timeouts each.
                if (d->pendingAddresses.contains(idCopy) &&
                    !d->pendingAddresses[idCopy].isEmpty()) {
                    qCInfo(lcApp) << "[ADDR-QUEUE] Address failed for" << idCopy
                                  << "— trying next in queue ("
                                  << d->pendingAddresses[idCopy].size() << "remaining)";
                    QTimer::singleShot(100, this, [this, idCopy]() {
                        tryNextPeerAddress(idCopy);
                    });
                    return;
                }

                // Queue empty — need a fresh DHT lookup.
                qCInfo(lcApp) << "[RELOOKUP] Peer" << id.value
                              << "needs relookup — queue exhausted, starting fresh DHT lookup.";
                d->pendingAddresses.remove(idCopy);
                // Mark the last attempted address as failed in DHT cache
                // so future loadDhtCache() deprioritizes it during its TTL.
                {
                    const Core::PeerAddress lastFailed =
                        d->lastAttemptedAddr.value(idCopy);
                    if (!lastFailed.host.isEmpty())
                        d->storage->markDhtAddressFailed(id, lastFailed);
                }
                d->storage->clearPeerAddress(id);

                // Fix 2: check OUR mailbox immediately — peer may have left us
                // a connection request while we were failing to reach them.
#if defined(SNAPCLONE_OPENDHT_BACKEND)
                if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
                    qCInfo(lcApp) << "[MAILBOX] Queue-exhausted check for connection requests";
                    dht->checkConnectionRequests(Core::UserId{ d->stableId });
                }
#endif // SNAPCLONE_OPENDHT_BACKEND
                // Only purge DHT cache after several failures — on the first
                // few retries the cached addrs are still worth trying. Purging
                // too early forces a cold live lookup every time.
                const int attemptsForPurge = d->relookupCount.value(idCopy, 0);
                if (attemptsForPurge >= 3)
                    d->storage->purgeDhtCacheForPeer(id);
                d->lastAttemptedAddr.remove(id.value);
                d->pendingDhtLookups.remove(id.value);
                d->network->suppressDiscovery(id, 30000);
                // Exponential backoff before relookup (2s→4s→8s→60s max).
                const int attempts = d->relookupCount.value(idCopy, 0);
                const int delayMs = qMin(2000 * (1 << qMin(attempts, 5)), 60000);
                qCInfo(lcApp) << "[RELOOKUP] Backoff" << delayMs << "ms (attempt"
                              << (attempts + 1) << "of" << Impl::kMaxRelookups << ")";
                emit peerConnectionStatus(Core::UserId{ idCopy },
                    QStringLiteral("relookup"),
                    tr("Re-scanning — attempt %1 of %2")
                        .arg(attempts + 1).arg(Impl::kMaxRelookups));
                // After 3 failed attempts, drop an encrypted connection request
                // in their DHT mailbox so they can reach us when they come online.
                if (attempts >= 1 && !d->mailboxPublished.contains(idCopy)) {
                    d->mailboxPublished.insert(idCopy);
                    // Re-publish after 10 min if still disconnected (mailbox TTL is 10 min)
                    const QString midCopy = idCopy;
                    QTimer::singleShot(10 * 60 * 1000, this, [this, midCopy]() {
                        if (!d->network->hasAnyLiveSession(Core::UserId{ midCopy }))
                            d->mailboxPublished.remove(midCopy); // allow re-publish next relookup
                    });
                    const Core::UserId targetId{ idCopy };
                    // Best addr for mailbox: STUN reflexive → selfAddr (ipify fallback)
                    const Core::PeerAddress refl = d->nat ? d->nat->reflexiveAddress()
                                                           : Core::PeerAddress{};
                    const QString myHost = !refl.host.isEmpty() ? refl.host
                                         : d->selfAddr.host;
                    const quint16 myPort = (refl.port > 0 && refl.port != 9876) ? refl.port
                                         : d->selfAddr.port > 0 ? d->selfAddr.port
                                         : 9876;
                    qCInfo(lcApp) << "[MAILBOX] Publishing connection request to"
                                  << idCopy << "— my addr:" << myHost << ":" << myPort;
                    emit peerConnectionStatus(Core::UserId{ idCopy },
                        QStringLiteral("mailbox"),
                        tr("Sending mailbox request…"));
                    const qint64 ts = QDateTime::currentSecsSinceEpoch();
                    // field[2] = TCP/WS addr (ipify) — for WebSocket connection attempts
                    // field[3] = UDP punch addr (STUN reflexive) — may differ from TCP addr
                    // refl is already computed above (used for myHost/myPort).
                    const QString tcpHost = d->selfAddr.host.isEmpty() ? myHost : d->selfAddr.host;
                    const quint16 tcpPort2 = d->selfAddr.port > 0 ? d->selfAddr.port : quint16(9876);
                    const QString udpHost = !refl.host.isEmpty() ? refl.host : tcpHost;
                    const quint16 udpPort = refl.port > 0 ? refl.port : tcpPort2;
                    // V32.37+ delivery fix: append LAN addr (6th field) so a
                    // same-WiFi peer connects directly instead of hairpinning the
                    // public IP. d->lanAddr is preserved across the STUN overwrite.
                    const QString lanStr2 = (!d->lanAddr.host.isEmpty()
                                             && d->lanAddr.host != tcpHost)
                        ? QStringLiteral("%1:%2").arg(d->lanAddr.host).arg(d->lanAddr.port)
                        : QString();
                    const QString plain = lanStr2.isEmpty()
                        ? QStringLiteral("CONNREQ\n%1\n%2:%3\n%4:%5\n%6\n")
                            .arg(d->stableId).arg(tcpHost).arg(tcpPort2)
                            .arg(udpHost).arg(udpPort).arg(ts)
                        : QStringLiteral("CONNREQ\n%1\n%2:%3\n%4:%5\n%6\n%7\n")
                            .arg(d->stableId).arg(tcpHost).arg(tcpPort2)
                            .arg(udpHost).arg(udpPort).arg(ts).arg(lanStr2);
                    const auto enc = d->crypto->encryptFor(targetId, plain.toUtf8());
                    if (!enc) {
                        qCWarning(lcApp) << "[MAILBOX] Cannot encrypt for" << idCopy
                                         << "— no public key yet. Will retry on next relookup"
                                         << "once DHT returns their key or direct SHAK completes.";
                        // Don't give up — the DHT lookup running in parallel may return
                        // their public key, enabling encryption on the next relookup cycle.
                        // Nothing more to do here; TCP connect attempt is already in flight.
                    } else if (d->stableId.isEmpty()) {
                        qCWarning(lcApp) << "[MAILBOX] stableId empty — cannot publish";
                    } else {
                        QByteArray blob;
                        blob.append(d->stableId.toUtf8());
                        blob.append('\n');
                        blob.append(enc->nonce);
                        blob.append(enc->ciphertext);
                        qCInfo(lcApp) << "[MAILBOX] Blob size:" << blob.size()
                                      << "nonce:" << enc->nonce.toHex()
                                      << "ciphertext:" << enc->ciphertext.size() << "bytes";
#if defined(SNAPCLONE_OPENDHT_BACKEND)
                        if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get()))
                            dht->publishConnectionRequest(targetId, blob);
                        else
                            qCWarning(lcApp) << "[MAILBOX] DhtDiscovery not available";
#endif // SNAPCLONE_OPENDHT_BACKEND
                    }
                }
                // V32.47: during a live call, stretch the relookup spacing so
                // the background DHT churn doesn't compete with the call audio.
                // The loop stays alive (self-healing); normal cadence resumes
                // automatically once the call ends.
                const int effDelayMs = callActive()
                    ? qMax(delayMs, static_cast<int>(Impl::kInCallRelookupMs))
                    : delayMs;
                QTimer::singleShot(effDelayMs, this, [this, idCopy]() {
                    connectByDhtId(idCopy);
                });
            }, Qt::QueuedConnection);
}


} // namespace SnapClone::Core
