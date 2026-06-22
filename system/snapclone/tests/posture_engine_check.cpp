// Standalone behavioral check for PostureEngine. Build/run in-tree or in the
// sandbox; exits 0 only if every transition fires as specified.
//   g++ -std=c++20 -fPIC $(pkg-config --cflags Qt6Core) -I include \
//       tests/posture_engine_check.cpp src/ui/posture/PostureEngine.cpp \
//       <moc_PostureEngine.cpp> $(pkg-config --libs Qt6Core) -o /tmp/pe && /tmp/pe
#include "ui/posture/PostureEngine.hpp"
#include <QCoreApplication>
#include <QEventLoop>
#include <QTimer>
#include <cstdio>
#include <cmath>

using SnapClone::UI::PostureEngine;

static int g_fails = 0;
static void check(bool ok, const char *what)
{
    std::printf("  [%s] %s\n", ok ? "PASS" : "FAIL", what);
    if (!ok) ++g_fails;
}
static void spin(int ms)
{
    QEventLoop loop;
    QTimer::singleShot(ms, &loop, &QEventLoop::quit);
    loop.exec();
}

int main(int argc, char **argv)
{
    QCoreApplication app(argc, argv);

    PostureEngine eng;
    PostureEngine::Config cfg;            // short timers for the test
    cfg.stillnessMs = 40; cfg.wakeMs = 10; cfg.fadeToDim1Ms = 5; cfg.fadeToDim2Ms = 5;
    eng.setConfig(cfg);

    // capture outputs
    double lastDim = -1; int dimCount = 0;
    bool   saver = false; int saverCount = 0;
    bool   gated = false; int gatedCount = 0;
    QObject::connect(&eng, &PostureEngine::dimTargetChanged,
                     [&](qreal o, int){ lastDim = o; ++dimCount; });
    QObject::connect(&eng, &PostureEngine::screensaverChanged,
                     [&](bool a){ saver = a; ++saverCount; });
    QObject::connect(&eng, &PostureEngine::gatedModeRequested,
                     [&](bool g){ gated = g; ++gatedCount; });

    auto nearEq = [](double a, double b){ return std::fabs(a - b) < 1e-6; };

    std::puts("PostureEngine behavioral check:");

    // 1. Arm + in a call, nothing covering → stays Normal, no gated yet.
    eng.setInCall(true);
    eng.setArmed(true);
    check(eng.state() == PostureEngine::State::Normal, "armed: starts Normal");
    check(dimCount == 0 && gatedCount == 0, "armed: nothing emitted yet");

    // 2. Proximity near → Dim (50%) + gated on.
    eng.setProximityNear(true);
    check(eng.state() == PostureEngine::State::Dim, "near: → Dim");
    check(nearEq(lastDim, 0.50), "near: dim target 0.50");
    check(gated == true, "near + in call: gated requested");

    // 3. Held still past stillnessMs → Screensaver (75%).
    spin(80);
    check(eng.state() == PostureEngine::State::Screensaver, "still: → Screensaver");
    check(nearEq(lastDim, 0.75), "still: dim target 0.75");
    check(saver == true, "still: screensaver on");

    // 4. Snap arrives while covered → wake (0) then re-dim (50%), back in Dim.
    eng.notifySnapArrived();
    check(nearEq(lastDim, 0.0), "snap: lights up to 0");
    check(saver == false, "snap: screensaver off");
    spin(40);
    check(eng.state() == PostureEngine::State::Dim, "snap: re-dims to Dim");
    check(nearEq(lastDim, 0.50), "snap: dim target back to 0.50");

    // 5. Motion resets the stillness countdown (no screensaver while moving).
    eng.setMoving(true);
    spin(80);
    check(eng.state() == PostureEngine::State::Dim, "moving: stays Dim (no screensaver)");
    eng.setMoving(false);
    spin(80);
    check(eng.state() == PostureEngine::State::Screensaver, "still again: → Screensaver");

    // 6. Proximity far → wake to Normal + gated off.
    eng.setProximityNear(false);
    check(eng.state() == PostureEngine::State::Normal, "far: → Normal");
    check(nearEq(lastDim, 0.0), "far: dim target 0.0");
    check(gated == false, "far: gated released");

    // 7. Disarm is inert from Normal; arming respects the master enable flag.
    eng.setArmed(false);
    cfg.enabled = false; eng.setConfig(cfg);
    eng.setArmed(true);
    check(eng.armed() == false, "disabled: refuses to arm");

    std::printf("\n%s (%d failures)\n", g_fails ? "FAILED" : "ALL PASS", g_fails);
    return g_fails ? 1 : 0;
}
