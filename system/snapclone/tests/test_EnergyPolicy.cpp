// ─── tests/test_EnergyPolicy.cpp ─────────────────────────────────────────────
//
// Truth table for the energy/timer-gating policy. Pure C++, no Qt — proves the
// decisions that ActivityGate broadcasts to periodic timers without needing a
// device, a screen, or a battery.
// ─────────────────────────────────────────────────────────────────────────────

#include "core/EnergyPolicy.hpp"

#include <cstdio>

using namespace SnapClone::Core;
using T = EnergyThermal;

static int g_pass = 0, g_fail = 0;

static void check(const char *name, bool got, bool want)
{
    if (got == want) { ++g_pass; std::printf("  PASS  %-52s %s\n", name, got ? "true" : "false"); }
    else { ++g_fail; std::printf("  FAIL  %-52s got %s want %s\n", name,
                                  got ? "true" : "false", want ? "true" : "false"); }
}
static void checkI(const char *name, int got, int want)
{
    if (got == want) { ++g_pass; std::printf("  PASS  %-52s %d\n", name, got); }
    else { ++g_fail; std::printf("  FAIL  %-52s got %d want %d\n", name, got, want); }
}

// helper to build a policy in one line
static EnergyPolicy mk(bool screen, bool fg, bool saver, T thermal)
{
    EnergyPolicy p;
    p.setScreenOn(screen);
    p.setAppForeground(fg);
    p.setBatterySaver(saver);
    p.setThermal(thermal);
    return p;
}

int main()
{
    std::printf("\nEnergyPolicy — interactive() gate (decorative / viewfinder timers)\n");
    std::printf("-----------------------------------------------------------------\n");
    check("screen on + foreground -> interactive",      mk(true,  true,  false, T::Normal).interactive(), true);
    check("screen OFF + foreground -> NOT interactive", mk(false, true,  false, T::Normal).interactive(), false);
    check("screen on + backgrounded -> NOT interactive",mk(true,  false, false, T::Normal).interactive(), false);
    check("screen off + backgrounded -> NOT interactive",mk(false,false, false, T::Normal).interactive(), false);

    std::printf("\nbackgroundWorkAllowed() gate (keepalive / reconnect / discovery)\n");
    std::printf("-----------------------------------------------------------------\n");
    check("normal -> allowed",            mk(true,  true,  false, T::Normal).backgroundWorkAllowed(),   true);
    check("backgrounded -> still allowed",mk(false, false, false, T::Normal).backgroundWorkAllowed(),   true);
    check("hot -> still allowed",         mk(true,  true,  false, T::Hot).backgroundWorkAllowed(),      true);
    check("CRITICAL -> NOT allowed",      mk(true,  true,  false, T::Critical).backgroundWorkAllowed(), false);

    std::printf("\ncadenceScale() — interval stretch for maintenance wakeups\n");
    std::printf("-----------------------------------------------------------------\n");
    checkI("interactive, normal -> 1",            mk(true,  true,  false, T::Normal).cadenceScale(),   1);
    checkI("screen off -> 2",                     mk(false, true,  false, T::Normal).cadenceScale(),   2);
    checkI("backgrounded -> 2",                   mk(true,  false, false, T::Normal).cadenceScale(),   2);
    checkI("interactive + battery-saver -> 2",    mk(true,  true,  true,  T::Normal).cadenceScale(),   2);
    checkI("screen off + battery-saver -> 4",     mk(false, true,  true,  T::Normal).cadenceScale(),   4);
    checkI("interactive + hot -> 2",              mk(true,  true,  false, T::Hot).cadenceScale(),      2);
    checkI("screen off + saver + hot -> 8 (cap)", mk(false, true,  true,  T::Hot).cadenceScale(),      8);
    checkI("CRITICAL -> 0 (paused)",              mk(true,  true,  false, T::Critical).cadenceScale(), 0);

    std::printf("\n-----------------------------------------------------------------\n");
    std::printf("EnergyPolicy: %d passed, %d failed\n\n", g_pass, g_fail);
    return g_fail == 0 ? 0 : 1;
}
