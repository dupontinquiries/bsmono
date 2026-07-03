#pragma once
// ─── DataBusClient.hpp ────────────────────────────────────────────────────────
//
// Qt wrapper around the snapclone-databus Unix socket protocol.
// Any widget can:
//   DataBusClient *bus = DataBusClient::instance();
//   bus->publish(MapPoint{...});          // send a point
//   bus->subscribe({"public_snap","weather_station"});  // get live updates
//   connect(bus, &DataBusClient::pointReceived, this, &MyWidget::onPoint);
//
// The singleton is started in AppController::initialize() alongside the server.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QLocalSocket>
#include <QJsonObject>
#include <QJsonArray>
#include <QRectF>
#include <QDateTime>

namespace SnapClone::Network {

// ── MapPoint ──────────────────────────────────────────────────────────────────

struct MapPoint {
    QString   id;
    QString   layer;         // "public_snap", "weather_station", etc.
    double    lat   = 0.0;
    double    lon   = 0.0;
    float     accuracyM = -1.f;
    QString   sourceApp;
    int       ttlSecs = 300;
    QJsonObject payload;
    QDateTime publishedAt;

    bool isAlive() const {
        if (ttlSecs == 0) return true;
        return publishedAt.secsTo(QDateTime::currentDateTimeUtc()) < ttlSecs;
    }

    QJsonObject toJson() const;
    static MapPoint fromJson(const QJsonObject &obj);
};

// ── DataBusClient ──────────────────────────────────────────────────────────────

class DataBusClient : public QObject
{
    Q_OBJECT
public:
    static DataBusClient *instance();

    /// Publish a map point. Returns the assigned point ID.
    QString publish(const MapPoint &point);

    /// Subscribe to specific layers. pointReceived will fire for new points.
    void subscribe(const QStringList &layers);

    /// Query a snapshot of in-TTL points synchronously.
    QList<MapPoint> list(const QStringList &layers,
                         const QRectF       &bbox  = {},
                         int                 limit = 200);

    bool isConnected() const;

signals:
    /// Fired when a subscribed layer receives a new point.
    void pointReceived(const SnapClone::Network::MapPoint &point);
    /// Fired when the bus connection is established.
    void connected();
    /// Fired when the bus connection is lost (will retry).
    void disconnected();

private slots:
    void onReadyRead();
    void onConnected();
    void onDisconnected();
    void tryReconnect();

private:
    explicit DataBusClient(QObject *parent = nullptr);
    void sendMessage(const QJsonObject &msg);
    void processLine(const QByteArray &line);
    void handlePush(const QJsonObject &obj);

    QLocalSocket *m_socket = nullptr;
    QByteArray    m_buf;
    QStringList   m_subscribedLayers;
    bool          m_autoReconnect = true;
};

} // namespace SnapClone::Network
