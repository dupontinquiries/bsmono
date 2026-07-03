#pragma once
// ─── Network/PollinatorDiscovery.hpp ──────────────────────────────────────────
// UDP multicast beacon for local peer discovery (Pollinator / Nearby screen).
//
// Broadcasts "SNAPCLONE_BEACON\nsc-ID\ndisplayName\n" every 2s to
// 224.0.0.251:9879 (LAN-only, TTL=1 so it never leaves the local network).
//
// Privacy: ONLY active while the Nearby screen is open.
//          No data leaves the local network (multicast TTL=1).
//          No DHT, no internet, no location data.
// ─────────────────────────────────────────────────────────────────────────────
#include "core/Types.hpp"
#include <QObject>
#include <memory>

namespace SnapClone::Network {

class PollinatorDiscovery : public QObject
{
    Q_OBJECT
public:
    explicit PollinatorDiscovery(QObject *parent = nullptr);
    ~PollinatorDiscovery() override;

    void setSelfId(const QString &id, const QString &displayName);

public slots:
    void start();  // connect to ContactsPage::beaconStarted
    void stop();   // connect to ContactsPage::beaconStopped

signals:
    void peerBeaconed(SnapClone::Core::UserId peerId, QString displayName, QString senderAddress);

private slots:
    void sendBeacon();
    void onReadyRead();

private:
    struct Impl;
    std::unique_ptr<Impl> d;
};

} // namespace SnapClone::Network
