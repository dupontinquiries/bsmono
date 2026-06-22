#include "ui/IncomingCallDialog.hpp"
#include "ui/Theme.hpp"
#include "ui/RingtoneSynth.hpp"

#include <QSettings>

#include <QVBoxLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QPushButton>
#include <QTimer>

#include <QAudioSink>
#include <QAudioFormat>
#include <QAudioDevice>
#include <QMediaDevices>
#include <QAudio>
#include <QBuffer>

namespace SnapClone::UI {

namespace {
// How long the ring stays up before auto-declining. PTT talk-spurts are short,
// but the conversation is still joinable after, so give the user a real window.
constexpr int kRingTimeoutMs = 30000;
} // namespace

IncomingCallDialog::IncomingCallDialog(const QString &callerName, QWidget *parent)
    : MobileDialog(parent)
{
    // A ringing call sits idle-handed while the user decides whether to answer,
    // so give it the long phone-call idle window. (Its own 30s ring timeout
    // still fires first to auto-decline.)
    setDismissContext(DismissContext::PhoneCall);

    buildUi(callerName);

    m_ringTimeout = new QTimer(this);
    m_ringTimeout->setSingleShot(true);
    m_ringTimeout->setInterval(kRingTimeoutMs);
    connect(m_ringTimeout, &QTimer::timeout, this, [this] { stopRingtone(); reject(); });
    m_ringTimeout->start();

    startRingtone();
}

IncomingCallDialog::~IncomingCallDialog()
{
    stopRingtone();   // safety net for close paths that bypass the buttons
}

void IncomingCallDialog::buildUi(const QString &callerName)
{
    setStyleSheet(QStringLiteral("QDialog { background: %1; border-radius: 18px; }")
                      .arg(QLatin1String(Theme::BG_CARD())));

    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(24, 22, 24, 22);
    root->setSpacing(4);

    auto *kicker = new QLabel(tr("INCOMING VOICE"), this);
    kicker->setStyleSheet(QStringLiteral("color: %1; font-size: 11px; letter-spacing: 2px;")
                              .arg(QLatin1String(Theme::TEXT_SECONDARY())));
    kicker->setAlignment(Qt::AlignHCenter);

    auto *who = new QLabel(callerName.isEmpty() ? tr("Unknown") : callerName, this);
    who->setStyleSheet(QStringLiteral("color: %1; font-size: 22px; font-weight: 600;")
                           .arg(QLatin1String(Theme::TEXT_PRIMARY())));
    who->setAlignment(Qt::AlignHCenter);
    who->setWordWrap(true);

    root->addWidget(kicker);
    root->addWidget(who);
    root->addSpacing(18);

    auto *btnRow = new QHBoxLayout();
    btnRow->setSpacing(12);

    const auto buttonCss = QStringLiteral(
        "QPushButton { background: %1; color: white; border: none; border-radius: 12px; "
        "padding: 13px 0; font-size: 15px; font-weight: 600; }");

    auto *declineBtn = new QPushButton(tr("Decline"), this);
    declineBtn->setCursor(Qt::PointingHandCursor);
    declineBtn->setStyleSheet(buttonCss.arg(QLatin1String(Theme::DANGER())));
    connect(declineBtn, &QPushButton::clicked, this, [this] { stopRingtone(); reject(); });

    auto *acceptBtn = new QPushButton(tr("Accept"), this);
    acceptBtn->setCursor(Qt::PointingHandCursor);
    acceptBtn->setStyleSheet(buttonCss.arg(QLatin1String(Theme::ACCENT())));
    connect(acceptBtn, &QPushButton::clicked, this, [this] { stopRingtone(); accept(); });

    btnRow->addWidget(declineBtn, 1);
    btnRow->addWidget(acceptBtn, 1);
    root->addLayout(btnRow);
}

void IncomingCallDialog::startRingtone()
{
    // Sounds settings: a muted ringtone rings silently (the dialog still shows).
    QSettings sounds;
    if (sounds.value(QStringLiteral("sounds/ringtoneMuted"), false).toBool())
        return;

    const QAudioDevice dev = QMediaDevices::defaultAudioOutput();
    if (dev.isNull()) return;   // no output device → silent ring

    QAudioFormat fmt;
    fmt.setSampleRate(44100);
    fmt.setChannelCount(1);
    fmt.setSampleFormat(QAudioFormat::Int16);
    if (!dev.isFormatSupported(fmt))
        fmt = dev.preferredFormat();   // fall back; the synth adapts to it

    // Synthesize the ring via the shared RingtoneSynth (same code the
    // ringtone_workshop tool uses). PROXIMITY EASTER-EGG HOOK: today we use the
    // default motif; to make the ring gain texture near a shared landmark, build
    // params here from a landmark-proximity lookup —
    //   auto p = Audio::RingtoneParams::defaults();
    //   p.noiseKernel = Audio::NoiseKernel::Shimmer;
    //   p.noiseIntensity = proximity01;            // 0 far … 1 at the landmark
    //   p.noiseSeed = qHash(nearestLandmarkId);    // per-landmark character
    // — and pass `p` instead of defaults(). (Wiring the LandmarkHash/Wayfinder
    // proximity feed is a follow-up; the synth already supports it.)
    namespace A = SnapClone::Audio;
    const A::SampleFmt sf = (fmt.sampleFormat() == QAudioFormat::Float)
                                ? A::SampleFmt::Float32 : A::SampleFmt::Int16;
    // Apply the user's ringtone gain (Sounds settings) on top of the default
    // master level (clamped 0–200%; renderMono clamps the final samples).
    A::RingtoneParams params = A::RingtoneParams::defaults();
    double g = sounds.value(QStringLiteral("sounds/ringtoneGain"), 100.0).toDouble() / 100.0;
    if (g < 0.0) g = 0.0; else if (g > 2.0) g = 2.0;
    params.master *= g;
    const QByteArray pcm = A::renderPcm(params,
                                        fmt.sampleRate()   > 0 ? fmt.sampleRate()   : 44100,
                                        fmt.channelCount() > 0 ? fmt.channelCount() : 1,
                                        sf);
    if (pcm.isEmpty()) return;

    m_ringPcm = new QBuffer(this);
    m_ringPcm->setData(pcm);
    if (!m_ringPcm->open(QIODevice::ReadOnly)) {
        m_ringPcm->deleteLater();
        m_ringPcm = nullptr;
        return;
    }

    m_sink = new QAudioSink(dev, fmt, this);
    m_ringtoneActive = true;
    // Loop: when the buffer drains (Idle), rewind and play again until stopped.
    connect(m_sink, &QAudioSink::stateChanged, this, [this](QAudio::State s) {
        if (s == QAudio::IdleState && m_ringtoneActive && m_ringPcm) {
            m_ringPcm->seek(0);
            m_sink->start(m_ringPcm);
        }
    });
    m_sink->start(m_ringPcm);
}

void IncomingCallDialog::stopRingtone()
{
    m_ringtoneActive = false;   // gate the loop restart in the stateChanged slot
    if (m_sink) {
        m_sink->stop();
        m_sink->deleteLater();
        m_sink = nullptr;
    }
    if (m_ringPcm) {
        m_ringPcm->close();
        m_ringPcm->deleteLater();
        m_ringPcm = nullptr;
    }
}

} // namespace SnapClone::UI
