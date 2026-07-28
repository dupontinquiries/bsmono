// ─── tests/test_TraceChannels.cpp ────────────────────────────────────────────
// The trace-channel policy is Qt-free precisely so it can be tested like this:
// plain compiler, no display, no app. Build standalone:
//   g++ -std=c++20 -I include tests/test_TraceChannels.cpp src/debug/TraceChannels.cpp
// ─────────────────────────────────────────────────────────────────────────────
#include "debug/TraceChannels.hpp"

// Exercise the real macros against a stub logger (see Trace.hpp): the point is
// to prove that a disabled channel does not evaluate its arguments.
#define SC_TRACE_TESTING 1
static int g_logged = 0;
static int g_evaluated = 0;
struct StubCat { };
static StubCat lcStub;
/// Streamable sink so the stub works for BOTH call shapes the codebase uses:
/// printf-style `qCInfo(cat, "fmt", args)` and stream-style `qCInfo(cat) << x`.
struct Sink { template <class T> Sink &operator<<(const T &) { return *this; } };
template <typename... A> Sink stubLog(const StubCat &, A &&...) { ++g_logged; return {}; }
#define qCInfo(...)  stubLog(__VA_ARGS__)
#define qCDebug(...) stubLog(__VA_ARGS__)
#include "debug/Trace.hpp"
static int expensive() { ++g_evaluated; return 7; }

#include <cassert>
#include <cstdio>
#include <iostream>
#include <set>
#include <string>

using namespace SnapClone::Debug;

static int g_failures = 0;
#define CHECK(cond, msg) do { \
    if (!(cond)) { std::cerr << "FAIL: " << (msg) << "  [" #cond "]\n"; ++g_failures; } \
} while (0)

static bool rulesSay(const std::string &rules, const std::string &line)
{
    return rules.find(line + "\n") != std::string::npos;
}

int main()
{
    // ── every channel is named, described, and owns at least one prefix ──────
    for (std::size_t i = 0; i < kChannelCount; ++i) {
        const auto c = static_cast<TraceChannel>(i);
        CHECK(std::string(name(c)).size() > 0, "channel has a name");
        CHECK(std::string(help(c)).size() > 10, "channel has a real description");
        CHECK(!prefixes(c).empty(), "channel owns categories");
    }

    // ── channels are DISJOINT: a category must never land in two channels with
    //    contradictory rules (the invariant that keeps filterRules coherent) ──
    {
        std::set<std::string> seen;
        for (std::size_t i = 0; i < kChannelCount; ++i) {
            for (const char *p : prefixes(static_cast<TraceChannel>(i))) {
                CHECK(seen.insert(p).second, std::string("prefix owned twice: ") + p);
            }
        }
    }

    // ── parsing: absolute, relative, aliases, and helpful errors ─────────────
    {
        TraceMask m;
        std::string err;
        CHECK(TraceMask::parse("input,layout", TraceMask::none(), &m, &err), "parses a list");
        CHECK(m.has(TraceChannel::Input) && m.has(TraceChannel::Layout), "list sets both");
        CHECK(!m.has(TraceChannel::Net), "list sets nothing else");

        CHECK(TraceMask::parse("ALL", TraceMask::none(), &m, &err), "case-insensitive 'all'");
        CHECK(m.bits() == TraceMask::all().bits(), "'all' enables every channel");

        CHECK(TraceMask::parse("none", TraceMask::all(), &m, &err), "'none' parses");
        CHECK(!m.any(), "'none' clears");

        // relative forms compose against the current mask
        TraceMask base = TraceMask::none();
        base.set(TraceChannel::Input, true);
        CHECK(TraceMask::parse("+net", base, &m, &err), "'+net' parses");
        CHECK(m.has(TraceChannel::Input) && m.has(TraceChannel::Net), "'+' keeps the base");
        CHECK(TraceMask::parse("-input", base, &m, &err), "'-input' parses");
        CHECK(!m.has(TraceChannel::Input), "'-' removes from the base");

        // an unknown name must fail LOUDLY and list the valid ones — the person
        // reading this is usually staring at an installed build with no docs
        CHECK(!TraceMask::parse("inpt", TraceMask::none(), &m, &err), "typo rejected");
        CHECK(err.find("inpt") != std::string::npos, "error quotes the bad name");
        CHECK(err.find("input") != std::string::npos, "error lists valid channels");

        // mixed absolute/relative is a typo, not a guess
        CHECK(!TraceMask::parse("input,-net", TraceMask::none(), &m, &err), "mixed form rejected");
    }

    // ── spec round-trip (settings persistence depends on it) ────────────────
    {
        TraceMask m;
        std::string err;
        for (const char *spec : {"none", "all", "input,net", "camera", "storage,voice,power"}) {
            CHECK(TraceMask::parse(spec, TraceMask::none(), &m, &err), "spec parses");
            TraceMask back;
            CHECK(TraceMask::parse(m.toSpec(), TraceMask::none(), &back, &err), "toSpec re-parses");
            CHECK(back.bits() == m.bits(), std::string("round-trip stable for ") + spec);
        }
    }

    // ── filter rules: complete, and the right way round ─────────────────────
    {
        TraceMask m;
        std::string err;
        CHECK(TraceMask::parse("input", TraceMask::none(), &m, &err), "parses for rules");
        const std::string rules = m.filterRules();

        // the enabled channel's categories are switched ON, wildcard included
        CHECK(rulesSay(rules, "snapclone.ui.swipenav.info=true"), "enabled: exact info rule");
        CHECK(rulesSay(rules, "snapclone.ui.swipenav.debug=true"), "enabled: exact debug rule");
        CHECK(rulesSay(rules, "snapclone.ui.swipenav.*.info=true"), "enabled: child wildcard");

        // a disabled channel is explicitly false, not merely absent — this is the
        // bug that made the old apply() reset unrelated categories: setFilterRules
        // REPLACES the rule set, so silence means "Qt default", not "unchanged"
        CHECK(rulesSay(rules, "snapclone.camera.info=false"), "disabled channels stated explicitly");
        CHECK(rulesSay(rules, "snapclone.dht.debug=false"), "every channel appears in the rules");

        // warnings/criticals are never governed by tracing
        CHECK(rules.find("warning") == std::string::npos, "tracing never touches warnings");
        CHECK(rules.find("critical") == std::string::npos, "tracing never touches criticals");

        // every channel's every prefix is represented
        for (std::size_t i = 0; i < kChannelCount; ++i) {
            for (const char *p : prefixes(static_cast<TraceChannel>(i))) {
                CHECK(rules.find(std::string(p) + ".info=") != std::string::npos,
                      std::string("rules cover ") + p);
            }
        }
    }

    // ── launch config: env, CLI, precedence, and the field-debug affordances ─
    {
        // env alone
        auto lt = parseLaunch("input,net", nullptr, {}, TraceMask::none());
        CHECK(lt.mask.has(TraceChannel::Input) && lt.mask.has(TraceChannel::Net), "env sets channels");
        CHECK(lt.error.empty(), "clean env parses without error");

        // CLI beats env (a typed flag is more current than a shell profile)
        lt = parseLaunch("net", nullptr, {"--trace=camera"}, TraceMask::none());
        CHECK(lt.mask.has(TraceChannel::Camera) && !lt.mask.has(TraceChannel::Net), "CLI wins over env");

        // separated form: --trace camera
        lt = parseLaunch(nullptr, nullptr, {"--trace", "camera"}, TraceMask::none());
        CHECK(lt.mask.has(TraceChannel::Camera), "--trace <value> form");

        // both flags exist for the two urgent cases: everything, and nothing
        lt = parseLaunch(nullptr, nullptr, {"--trace-all"}, TraceMask::none());
        CHECK(lt.mask.bits() == TraceMask::all().bits(), "--trace-all");
        TraceMask saved = TraceMask::all();
        lt = parseLaunch(nullptr, nullptr, {"--no-trace"}, saved);
        CHECK(!lt.mask.any() && lt.explicitOff, "--no-trace overrides persisted state");

        // persisted settings are the starting point when nothing is passed
        TraceMask persisted;
        persisted.set(TraceChannel::Storage, true);
        lt = parseLaunch(nullptr, nullptr, {}, persisted);
        CHECK(lt.mask.has(TraceChannel::Storage), "persisted channels survive a restart");

        // relative CLI composes with persisted state
        lt = parseLaunch(nullptr, nullptr, {"--trace=+voice"}, persisted);
        CHECK(lt.mask.has(TraceChannel::Storage) && lt.mask.has(TraceChannel::Voice),
              "relative CLI adds to persisted");

        // minutes, help, and a bad spec that reports rather than guesses
        lt = parseLaunch(nullptr, "45", {}, TraceMask::none());
        CHECK(lt.minutes == 45, "env minutes");
        lt = parseLaunch(nullptr, nullptr, {"--trace-minutes=90"}, TraceMask::none());
        CHECK(lt.minutes == 90, "CLI minutes");
        lt = parseLaunch(nullptr, nullptr, {"--trace-help"}, TraceMask::none());
        CHECK(lt.showHelp, "--trace-help requests help");
        lt = parseLaunch(nullptr, nullptr, {"--trace=nope"}, TraceMask::none());
        CHECK(!lt.error.empty(), "a bad CLI spec reports an error");
    }

    // ── the live gate + expiry ──────────────────────────────────────────────
    {
        setLiveMask(TraceMask::none());
        CHECK(!traceEnabled(TraceChannel::Input), "default off: nothing traces");
        TraceMask m;
        m.set(TraceChannel::Input, true);
        setLiveMask(m);
        CHECK(traceEnabled(TraceChannel::Input), "live mask enables its channel");
        CHECK(!traceEnabled(TraceChannel::Net), "and only its channel");
        CHECK(liveMask().bits() == m.bits(), "live mask reads back");
        setLiveMask(TraceMask::none());

        CHECK(!traceExpired(1000, 0), "deadline 0 never expires (until-I-turn-it-off)");
        CHECK(!traceExpired(999, 1000), "before the deadline, still tracing");
        CHECK(traceExpired(1000, 1000), "at the deadline, expired");
        CHECK(traceExpired(5000, 1000), "past the deadline, expired");
    }

    // ── help text mentions every channel and all three enable paths ─────────
    {
        const std::string h = helpText();
        for (std::size_t i = 0; i < kChannelCount; ++i) {
            CHECK(h.find(name(static_cast<TraceChannel>(i))) != std::string::npos,
                  "help lists every channel");
        }
        CHECK(h.find("--trace=") != std::string::npos, "help shows the CLI form");
        CHECK(h.find("SNAPCLONE_TRACE") != std::string::npos, "help shows the env form");
        CHECK(h.find("Settings") != std::string::npos, "help shows the in-app form");
    }

    // ── the cost model: a disabled channel must not evaluate its arguments ──
    // This is the whole justification for the macros over a bare qCInfo, which
    // formats its arguments BEFORE Qt decides to drop the message.
    {
        setLiveMask(TraceMask::none());
        g_logged = g_evaluated = 0;
        for (int i = 0; i < 1000; ++i) {
            SC_TRACE(Input, lcStub, "[WHEEL] v=%d", expensive());
        }
        CHECK(g_logged == 0, "channel off: nothing is logged");
        CHECK(g_evaluated == 0, "channel off: arguments are NEVER evaluated");

        TraceMask m;
        m.set(TraceChannel::Input, true);
        setLiveMask(m);
        g_logged = g_evaluated = 0;
        for (int i = 0; i < 10; ++i) {
            SC_TRACE(Input, lcStub, "[WHEEL] v=%d", expensive());
        }
        CHECK(g_logged == 10, "channel on: every line is logged");
        CHECK(g_evaluated == 10, "channel on: arguments are evaluated exactly once");

        // the stream form (`SC_TRACE_S(chan, cat) << ...`) gates identically
        setLiveMask(TraceMask::none());
        g_logged = g_evaluated = 0;
        SC_TRACE_S(Layout, lcStub) << "modal tap" << expensive();
        CHECK(g_logged == 0 && g_evaluated == 0, "stream form: off means nothing runs");
        TraceMask lm;
        lm.set(TraceChannel::Layout, true);
        lm.set(TraceChannel::Input, true);
        setLiveMask(lm);
        SC_TRACE_S(Layout, lcStub) << "modal tap" << expensive();
        CHECK(g_logged == 1 && g_evaluated == 1, "stream form: on means it runs once");

        // a different channel stays silent even while Input is live
        g_logged = 0;
        SC_TRACE(Net, lcStub, "should not appear %d", expensive());
        CHECK(g_logged == 0, "other channels remain off");

        // SC_TRACE_HOT compiles to nothing unless SNAPCLONE_HOT_TRACE is set
        g_logged = g_evaluated = 0;
        SC_TRACE_HOT(Input, lcStub, "hot %d", expensive());
#ifdef SNAPCLONE_HOT_TRACE
        CHECK(g_logged == 1, "hot trace fires in a diagnostic build");
#else
        CHECK(g_logged == 0 && g_evaluated == 0,
              "hot trace compiles out entirely in a production build");
#endif
        setLiveMask(TraceMask::none());
    }

    if (g_failures == 0) std::cout << "test_TraceChannels: all checks passed\n";
    else std::cout << "test_TraceChannels: " << g_failures << " FAILURES\n";
    return g_failures == 0 ? 0 : 1;
}
