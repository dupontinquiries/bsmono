#include "network/DataBusClient.hpp"
#include <QLocalSocket>
#include <QJsonDocument>
#include <QJsonArray>
#include <QTimer>
#include <QLoggingCategory>
#include <QStandardPaths>

Q_LOGGING_CATEGORY(lcBus, "snapclone.databus")

namespace SnapClone::Network {

// ── MapPoint serialisation ────────────────────────────────────────────────────

QJsonObject MapPoint::toJson() const
{
    QJsonObject obj;
    obj[QStringLiteral("id")]          = id;
    obj[QStringLiteral("layer")]       = layer;
    obj[QStringLiteral("lat")]         = lat;
    obj[QStringLiteral("lon")]         = lon;
    obj[QStringLiteral("accuracy_m")]  = static_cast<double>(accuracyM);
    obj[QStringLiteral("source_app")]  = sourceApp;
    obj[QStringLiteral("ttl_secs")]    = ttlSecs;
    obj[QStringLiteral("payload")]     = payload;
    obj[QStringLiteral("published_at")]= publishedAt.toString(Qt::ISODateWithMs);
    return obj;
}

MapPoint MapPoint::fromJson(const QJsonObject &obj)
{
    MapPoint p;
    p.id          = obj[QStringLiteral("id")].toString();
    p.layer       = obj[QStringLiteral("layer")].toString();
    p.lat         = obj[QStringLiteral("lat")].toDouble();
    p.lon         = obj[QStringLiteral("lon")].toDouble();
    p.accuracyM   = static_cast<float>(obj[QStringLiteral("accuracy_m")].toDouble(-1.0));
    p.sourceApp   = obj[QStringLiteral("source_app")].toString();
    p.ttlSecs     = obj[QStringLiteral("ttl_secs")].toInt(300);
    p.payload     = obj[QStringLiteral("payload")].toObject();
    p.publishedAt = QDateTime::fromString(
        obj[QStringLiteral("published_at")].toString(), Qt::ISODateWithMs);
    if (!p.publishedAt.isValid())
        p.publishedAt = QDateTime::currentDateTimeUtc();
    return p;
}

// ── DataBusClient singleton ──────────────────────────────────────────────────

DataBusClient *DataBusClient::instance()
{
    static DataBusClient *inst = new DataBusClient(nullptr);
    return inst;
}

DataBusClient::DataBusClient(QObject *parent)
    : QObject(parent)
    , m_socket(new QLocalSocket(this))
{
    connect(m_socket, &QLocalSocket::readyRead,    this, &DataBusClient::onReadyRead);
    connect(m_socket, &QLocalSocket::connected,    this, &DataBusClient::onConnected);
    connect(m_socket, &QLocalSocket::disconnected, this, &DataBusClient::onDisconnected);

    // Try to connect to the bus socket
    const QString runtime = qEnvironmentVariable("XDG_RUNTIME_DIR",
                                                  QStringLiteral("/tmp"));
    m_socket->connectToServer(runtime + QStringLiteral("/snapclone-bus.sock"));
}

void DataBusClient::onConnected()
{
    qCInfo(lcBus) << "DataBus connected";
    // Re-subscribe to any previously subscribed layers
    if (!m_subscribedLayers.isEmpty())
        subscribe(m_subscribedLayers);
    emit connected();
}

void DataBusClient::onDisconnected()
{
    qCWarning(lcBus) << "DataBus disconnected";
    emit disconnected();
    if (m_autoReconnect)
        QTimer::singleShot(3000, this, &DataBusClient::tryReconnect);
}

void DataBusClient::tryReconnect()
{
    if (m_socket->state() != QLocalSocket::ConnectedState) {
        const QString runtime = qEnvironmentVariable("XDG_RUNTIME_DIR", QStringLiteral("/tmp"));
        m_socket->connectToServer(runtime + QStringLiteral("/snapclone-bus.sock"));
    }
}

bool DataBusClient::isConnected() const
{
    return m_socket->state() == QLocalSocket::ConnectedState;
}

void DataBusClient::sendMessage(const QJsonObject &msg)
{
    if (!isConnected()) {
        qCWarning(lcBus) << "DataBus not connected — dropping message";
        return;
    }
    QByteArray data = QJsonDocument(msg).toJson(QJsonDocument::Compact);
    data.append('\n');
    m_socket->write(data);
}

QString DataBusClient::publish(const MapPoint &point)
{
    QJsonObject msg;
    msg[QStringLiteral("op")]    = QStringLiteral("pub");
    msg[QStringLiteral("point")] = point.toJson();
    sendMessage(msg);
    // ID is returned async via ack; return the local ID for tracking
    return point.id.isEmpty() ? QStringLiteral("pending") : point.id;
}

void DataBusClient::subscribe(const QStringList &layers)
{
    m_subscribedLayers = layers;
    QJsonObject msg;
    msg[QStringLiteral("op")]     = QStringLiteral("sub");
    msg[QStringLiteral("layers")] = QJsonArray::fromStringList(layers);
    sendMessage(msg);
}

QList<MapPoint> DataBusClient::list(const QStringList &layers,
                                     const QRectF       &bbox,
                                     int                 limit)
{
    QJsonObject msg;
    msg[QStringLiteral("op")]     = QStringLiteral("list");
    msg[QStringLiteral("layers")] = QJsonArray::fromStringList(layers);
    msg[QStringLiteral("limit")]  = limit;
    if (!bbox.isEmpty()) {
        QJsonArray b;
        b << bbox.top() << bbox.left() << bbox.bottom() << bbox.right();
        msg[QStringLiteral("bbox")] = b;
    }
    sendMessage(msg);

    // Wait synchronously for reply (max 2s) — this is fine since it's the main thread
    // and the server is local (no network round-trip).
    QList<MapPoint> result;
    if (m_socket->waitForReadyRead(2000)) {
        m_buf += m_socket->readAll();
        while (true) {
            const qsizetype nl = m_buf.indexOf('\n');
            if (nl < 0) break;
            const QByteArray line = m_buf.left(nl);
            m_buf.remove(0, nl + 1);
            const QJsonObject obj = QJsonDocument::fromJson(line).object();
            if (obj[QStringLiteral("op")].toString() == QLatin1String("points")) {
                for (const QJsonValue &v : obj[QStringLiteral("items")].toArray())
                    result.append(MapPoint::fromJson(v.toObject()));
                break;
            }
        }
    }
    return result;
}

void DataBusClient::onReadyRead()
{
    m_buf += m_socket->readAll();
    while (true) {
        const qsizetype nl = m_buf.indexOf('\n');
        if (nl < 0) break;
        const QByteArray line = m_buf.left(nl);
        m_buf.remove(0, nl + 1);
        processLine(line);
    }
}

void DataBusClient::processLine(const QByteArray &line)
{
    const QJsonObject obj = QJsonDocument::fromJson(line).object();
    const QString op = obj[QStringLiteral("op")].toString();
    if (op == QLatin1String("push")) {
        handlePush(obj[QStringLiteral("point")].toObject());
    } else if (op == QLatin1String("err")) {
        qCWarning(lcBus) << "DataBus error:" << obj[QStringLiteral("message")].toString();
    }
    // ack, pong: silently ignore
}

void DataBusClient::handlePush(const QJsonObject &obj)
{
    const MapPoint point = MapPoint::fromJson(obj);
    if (!point.isAlive()) return;
    qCDebug(lcBus) << "DataBus push:" << point.layer << "from" << point.sourceApp;
    emit pointReceived(point);
}

} // namespace SnapClone::Network
