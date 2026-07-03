#pragma once
// ─── ui/IncomingCallDialog.hpp ────────────────────────────────────────────────
//
// V32.40+: the incoming voice-call ring. Shown when a voice/floor packet arrives
// for a conversation whose thread isn't open (see InboxWidget's incoming-call
// flow). A calm bottom sheet with the caller's name and Accept / Decline, plus a
// looped synthesized ringtone.
//
// It carries no call state of its own — it's a pure prompt. Accept maps to
// QDialog::accept() (→ accepted()), Decline and the ring timeout both map to
// reject() (→ rejected()); the owner (InboxWidget) connects those to open the
// conversation in-call or drop the buffered packets. Built on MobileDialog so it
// inlines as a bottom sheet on mobile and floats over whatever page is visible.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/MobileDialog.hpp"
#include <QString>

QT_BEGIN_NAMESPACE
class QTimer;
class QAudioSink;
class QBuffer;
QT_END_NAMESPACE

namespace SnapClone::UI {

class IncomingCallDialog : public MobileDialog {
    Q_OBJECT
public:
    explicit IncomingCallDialog(const QString &callerName, QWidget *parent = nullptr);
    ~IncomingCallDialog() override;

private:
    void buildUi(const QString &callerName);

    /// V32.40+: looped synthesized ring — a short three-note motif with a
    /// feedback-comb echo and a fade, generated as PCM and played on the default
    /// audio output. No asset; the synth adapts to the device's format. Silent
    /// no-op if no output device is available. stopRingtone() is idempotent.
    void startRingtone();
    void stopRingtone();

    QTimer     *m_ringTimeout    = nullptr;   // auto-decline if the ring is ignored
    QAudioSink *m_sink           = nullptr;
    QBuffer    *m_ringPcm        = nullptr;   // one ring cycle, looped on drain
    bool        m_ringtoneActive = false;
};

} // namespace SnapClone::UI
