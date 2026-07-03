#pragma once
// ─── QrScanDialog.hpp ─────────────────────────────────────────────────────────
//
// QR code scanner popup for adding contacts.
//
// # UX flow
//
// 1. User taps the QR button (📷) in the Add Contact row.
// 2. QrScanDialog opens as a MobileDialog (bottom sheet on phone, window on desktop).
// 3. It requests a frame stream from CameraWidget via signal — CameraWidget
//    emits rawFrameAvailable() at ~8fps for scanning (throttled from its normal
//    30fps to save battery during scan).
// 4. Each frame is decoded by a minimal QR finder (no library needed —
//    we only need to recognise our own sc-ID format: "sc-[8 hex chars]",
//    "sc-[hex]:[port]", or "snapclone://[sc-id]:[port]").
// 5. On successful decode, the dialog emits scannedContact(scId, port) and
//    closes itself. The ContactsPage connects this to addFriendRequested.
// 6. If the camera isn't available, the dialog shows a manual-entry fallback.
//
// # QR format
//
// The QR codes produced by SettingsPage contain one of:
//   "sc-aabbccdd"                — sc-ID only, default port 9876
//   "sc-aabbccdd:9876"           — sc-ID + explicit port
//   "snapclone://sc-aabbccdd:9876"  — full URI (future-proof)
//
// The scanner only needs to find this pattern in a decoded QR payload.
// We use a simplified QR finder: scan the image for the three finder-pattern
// squares (top-left, top-right, bottom-left) and decode the timing/data
// modules. For our short payloads (Version 1–3 QR codes) this is reliable.
//
// Fallback: if Qt doesn't have camera access, show a text field where the
// user can paste a QR payload or sc-ID directly.

#include "ui/MobileDialog.hpp"
#include <QVideoFrame>
#include <QString>
#include <QImage>
#include <QTimer>
#include <functional>

QT_BEGIN_NAMESPACE
class QLabel;
class QPushButton;
class QLineEdit;
QT_END_NAMESPACE

namespace SnapClone::UI {

class CameraWidget;

class QrScanDialog : public MobileDialog
{
    Q_OBJECT
public:
    explicit QrScanDialog(CameraWidget *camera, QWidget *parent = nullptr);

public:
    void open() override;
    void reject() override;

signals:
    /// Emitted when a valid sc-ID QR code is decoded.
    /// port is 0 if not encoded in the QR.
    void scannedContact(const QString &scId, quint16 port);

    /// Ask CameraWidget to start emitting scan frames (throttled).
    void scanStartRequested();
    /// Ask CameraWidget to stop emitting scan frames.
    void scanStopRequested();

public slots:
    /// Called by CameraWidget with each scan frame (at ~8fps).
    void onScanFrame(const QImage &frame);

private:
    void buildUi();
    void startScan();
    void stopScan();
    void tryDecodeFrame(const QImage &frame);
    bool tryDecodeQr(const QImage &frame, QString &outPayload);
    bool parseSnapclonePayload(const QString &payload, QString &outScId, quint16 &outPort);
    void onDecoded(const QString &scId, quint16 port);
    void showFallbackUi();

    CameraWidget  *m_camera;
    QLabel        *m_viewfinder  = nullptr;
    QLabel        *m_statusLabel = nullptr;
    QLineEdit     *m_manualEdit  = nullptr;
    QPushButton   *m_manualBtn   = nullptr;
    QTimer        *m_scanTimer   = nullptr;
    bool           m_scanning    = false;
    bool           m_decoded     = false;
    int            m_frameCount  = 0;    // for throttle (process 1 in N frames)
};

} // namespace SnapClone::UI
