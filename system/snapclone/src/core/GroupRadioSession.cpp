#include "core/GroupRadioSession.hpp"

#include <QTimer>
#include <QDateTime>
#include <utility>

extern "C" {
#include "group/snapclone_group_ffi.h"
#include "audio/snapclone_audio_ffi.h"
}

namespace SnapClone::Core {

namespace {
constexpr int kTickMs = 120;                  // floor tick + transmit re-evaluation cadence
constexpr std::uint64_t kReassertMs = 500;    // keepalive broadcast interval (< floor timeout)
constexpr std::uint64_t kFloorHoldTimeoutMs = 1500; // drop a holder gone silent this long
constexpr std::uint64_t kFloorMaxHoldMs = 30000;    // fairness cap on a single turn

inline int modeRaw(TalkMode m) { return static_cast<int>(m); }
} // namespace

GroupRadioSession::GroupRadioSession(QString selfId, QObject *parent)
    : QObject(parent)
    , m_selfId(std::move(selfId))
    , m_logic(kReassertMs)
{
    const QByteArray id = m_selfId.toUtf8();
    m_floor = sc_group_floor_new(reinterpret_cast<const uint8_t *>(id.constData()),
                                 static_cast<size_t>(id.size()),
                                 kFloorHoldTimeoutMs, kFloorMaxHoldMs);
    m_gate = sc_radio_gate_new_default();

    m_tick = new QTimer(this);
    m_tick->setInterval(kTickMs);
    connect(m_tick, &QTimer::timeout, this, &GroupRadioSession::evaluate);
}

GroupRadioSession::~GroupRadioSession()
{
    if (m_floor) sc_group_floor_free(m_floor);
    if (m_gate)  sc_radio_gate_free(m_gate);
}

quint64 GroupRadioSession::nowMs() const
{
    // Monotonic ms since start(); 0 before the clock is running.
    return m_clock.isValid() ? static_cast<quint64>(m_clock.elapsed()) : 0;
}

void GroupRadioSession::start()
{
    if (!m_clock.isValid()) m_clock.start();
    if (m_tick && !m_tick->isActive()) m_tick->start();
}

void GroupRadioSession::stop()
{
    if (m_tick) m_tick->stop();
    // Release the floor if we were holding it, and tell peers.
    if (m_logic.reset()) {
        if (m_floor) sc_group_floor_release(m_floor, nowMs());
        Q_EMIT floorReleaseRequested(m_selfId);
    }
    m_claimTs = 0;
    if (m_gate) sc_radio_gate_reset(m_gate);
    setTransmitting(false);
    refreshHolder();
}

void GroupRadioSession::setTalkMode(TalkMode mode)
{
    if (m_mode == mode) return;
    m_mode = mode;
    // Apply immediately rather than waiting for the next tick.
    evaluate();
}

void GroupRadioSession::pressTalk()
{
    m_pttHeld = true;
    evaluate();
}

void GroupRadioSession::releaseTalk()
{
    m_pttHeld = false;
    evaluate();
}

void GroupRadioSession::onVoiceActivity(float likelihood)
{
    m_lastVad = likelihood;
    // VOX reacts per frame; PTT / Continuous / Off ignore the VAD in the gate,
    // so only bother re-evaluating off-tick when actually voice-activated.
    if (m_mode == TalkMode::VoiceActivated) evaluate();
}

void GroupRadioSession::onPeerFloorAssert(const QString &holder, quint64 claimTs)
{
    if (!m_floor) return;
    const QByteArray h = holder.toUtf8();
    sc_group_floor_remote_assert(m_floor,
                                 reinterpret_cast<const uint8_t *>(h.constData()),
                                 static_cast<size_t>(h.size()),
                                 claimTs, nowMs());
    // A peer taking the floor can preempt us — re-evaluate and refresh the UI.
    evaluate();
}

void GroupRadioSession::onPeerFloorRelease(const QString &holder)
{
    if (!m_floor) return;
    const QByteArray h = holder.toUtf8();
    sc_group_floor_remote_release(m_floor,
                                  reinterpret_cast<const uint8_t *>(h.constData()),
                                  static_cast<size_t>(h.size()));
    evaluate();
}

void GroupRadioSession::evaluate()
{
    if (!m_floor || !m_gate) return;
    const quint64 now = nowMs();

    // 1. Expire any holder that grabbed the floor then went silent / vanished.
    sc_group_floor_tick(m_floor, now);

    // 2. Does the local user intend to talk this instant? This also advances the
    //    gate's squelch state machine for VOX; it's trivial for PTT/Continuous/Off.
    const bool wants = sc_radio_gate_update(m_gate, modeRaw(m_mode),
                                            m_pttHeld ? 1 : 0, m_lastVad, now) > 0;

    // 3. If we want to talk, try to grab the floor (or keepalive if we hold it).
    //    Our claim timestamp is wall-clock so it's comparable across devices for
    //    the contention tiebreak, and stays stable for as long as we keep wanting
    //    it (claim age = time since we first wanted to talk → fair ordering).
    bool hold = false;
    if (wants) {
        if (m_claimTs == 0)
            m_claimTs = static_cast<quint64>(QDateTime::currentMSecsSinceEpoch());
        hold = sc_group_floor_request(m_floor, m_claimTs, now) > 0;
    } else {
        m_claimTs = 0; // dropped intent → the next grab starts a fresh claim
    }

    // 4. Pure orchestration: transmit? (re)assert? release?
    const RadioTransmitDecision d = m_logic.step(wants, hold, now);
    switch (d.action) {
    case FloorBroadcast::Assert:
        Q_EMIT floorAssertRequested(m_selfId, m_claimTs);
        break;
    case FloorBroadcast::Release:
        sc_group_floor_release(m_floor, now);
        Q_EMIT floorReleaseRequested(m_selfId);
        break;
    case FloorBroadcast::None:
        break;
    }

    setTransmitting(d.transmit);
    refreshHolder();
}

void GroupRadioSession::setTransmitting(bool on)
{
    if (m_transmitting == on) return;
    m_transmitting = on;
    Q_EMIT transmitStateChanged(on);
}

void GroupRadioSession::refreshHolder()
{
    if (!m_floor) return;
    char buf[128];
    size_t n = 0;
    const GroupFfiError e = sc_group_floor_current_holder(
        m_floor, reinterpret_cast<uint8_t *>(buf), sizeof(buf), &n);

    QString holder; // empty = floor open (NOT_FOUND) or an id too long to fit
    if (e == GROUP_FFI_OK && n <= sizeof(buf))
        holder = QString::fromUtf8(buf, static_cast<int>(n));

    if (holder != m_holder) {
        m_holder = holder;
        Q_EMIT floorHolderChanged(m_holder);
    }
}

} // namespace SnapClone::Core
