// ─── debug/TraceChannels.cpp ─────────────────────────────────────────────────
// Qt-free implementation of the trace-channel policy (see the header for the
// rationale and the cost model). Unit-tested by tests/test_TraceChannels.cpp
// with a plain compiler — no Qt, no display, no app.
// ─────────────────────────────────────────────────────────────────────────────
#include "debug/TraceChannels.hpp"

#include <algorithm>
#include <atomic>
#include <cctype>

namespace SnapClone::Debug {
namespace {

struct ChannelDef {
    const char *name;
    const char *help;
    std::vector<const char *> prefixes;
};

// Category prefixes are grouped by WHAT SOMEONE IS DEBUGGING, not by source
// directory: a person chasing a swipe bug wants gesture + swipenav + confirm
// together and nothing else. Each prefix covers itself and its `.` children.
const std::vector<ChannelDef> &defs()
{
    static const std::vector<ChannelDef> d = {
        {"input",   "swipe/wheel/gesture classification and hand-off",
                    {"snapclone.ui.swipenav", "snapclone.ui.gesture", "snapclone.ux"}},
        {"layout",  "virtual keyboard, modal stack, dialog geometry",
                    {"snapclone.ui.vkeyboard", "snapclone.ui.modal", "snapclone.ui.modaltest",
                     "snapclone.dialog"}},
        {"camera",  "capture pipeline, focus/VCM, camera registry",
                    {"snapclone.camera", "snapclone.ui.camera"}},
        {"media",   "encode/record/transcode, viewer, thumbnails, export",
                    {"snapclone.video", "snapclone.viewer", "snapclone.ui.thumbs",
                     "snapclone.export"}},
        {"net",     "sockets, NAT/UPnP, relays, gossip, routing",
                    {"snapclone.network", "snapclone.nat", "snapclone.upnp", "snapclone.udpchan",
                     "snapclone.websocket", "snapclone.gossip", "snapclone.pollinator",
                     "snapclone.direct", "snapclone.mwrelay", "snapclone.netstats",
                     "snapclone.tiered", "snapclone.relay", "snapclone.router",
                     "snapclone.firewall"}},
        {"dht",     "DHT lookups plus identity/account resolution",
                    {"snapclone.dht", "snapclone.net.microdht", "snapclone.identity",
                     "snapclone.accounts", "snapclone.jami"}},
        {"storage", "on-disk stores, inbox/outbox, databus, session",
                    {"snapclone.storage", "snapclone.inbox", "snapclone.outbox",
                     "snapclone.databus", "snapclone.session"}},
        {"voice",   "voice capture/playback and group-voice coordination",
                    {"snapclone.voice", "snapclone.ui.voicecoord"}},
        {"ui",      "ambient visuals, conversation views, icons, wayfinder",
                    {"snapclone.ui.ambient", "snapclone.ui.conv", "snapclone.icon",
                     "snapclone.wayfinder"}},
        {"power",   "power and sleep-state policy",
                    {"snapclone.power", "snapclone.sleepstate"}},
    };
    return d;
}

std::string lower(std::string s)
{
    std::transform(s.begin(), s.end(), s.begin(),
                   [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    return s;
}

std::vector<std::string> splitSpec(const std::string &s)
{
    std::vector<std::string> out;
    std::string cur;
    for (char c : s) {
        if (c == ',' || c == ' ' || c == ';' || c == '\t') {
            if (!cur.empty()) { out.push_back(cur); cur.clear(); }
        } else {
            cur.push_back(c);
        }
    }
    if (!cur.empty()) out.push_back(cur);
    return out;
}

std::string knownNames()
{
    std::string s;
    for (std::size_t i = 0; i < kChannelCount; ++i) {
        if (i) s += ", ";
        s += defs()[i].name;
    }
    return s;
}

std::atomic<std::uint32_t> g_live{0};

} // namespace

const char *name(TraceChannel c) noexcept
{
    const auto i = static_cast<std::size_t>(c);
    return i < kChannelCount ? defs()[i].name : "";
}

const char *help(TraceChannel c) noexcept
{
    const auto i = static_cast<std::size_t>(c);
    return i < kChannelCount ? defs()[i].help : "";
}

std::vector<const char *> prefixes(TraceChannel c)
{
    const auto i = static_cast<std::size_t>(c);
    return i < kChannelCount ? defs()[i].prefixes : std::vector<const char *>{};
}

TraceMask TraceMask::all()
{
    return TraceMask{(1u << kChannelCount) - 1u};
}

void TraceMask::set(TraceChannel c, bool on)
{
    const auto bit = 1u << static_cast<int>(c);
    if (on) m_bits |= bit; else m_bits &= ~bit;
}

bool TraceMask::parse(const std::string &spec, TraceMask base, TraceMask *out, std::string *error)
{
    const auto tokens = splitSpec(lower(spec));
    if (tokens.empty()) { if (out) *out = TraceMask::none(); return true; }

    // Relative form only when EVERY token is signed; a mixed spec is almost
    // certainly a typo and silently guessing would waste the debugger's time.
    const bool anySigned = std::any_of(tokens.begin(), tokens.end(),
        [](const std::string &t) { return t[0] == '+' || t[0] == '-'; });
    const bool allSigned = std::all_of(tokens.begin(), tokens.end(),
        [](const std::string &t) { return t[0] == '+' || t[0] == '-'; });
    if (anySigned && !allSigned) {
        if (error) *error = "mix of absolute and +/- channel names in \"" + spec
                          + "\"; use either \"input,net\" or \"+input,-net\"";
        return false;
    }

    TraceMask m = allSigned ? base : TraceMask::none();
    for (const auto &tok : tokens) {
        bool add = true;
        std::string word = tok;
        if (word[0] == '+' || word[0] == '-') { add = (word[0] == '+'); word = word.substr(1); }
        if (word.empty()) continue;

        if (word == "all") {
            m = add ? TraceMask::all() : TraceMask::none();
            continue;
        }
        if (word == "none" || word == "off") { m = TraceMask::none(); continue; }

        bool found = false;
        for (std::size_t i = 0; i < kChannelCount; ++i) {
            if (word == defs()[i].name) {
                m.set(static_cast<TraceChannel>(i), add);
                found = true;
                break;
            }
        }
        if (!found) {
            if (error) *error = "unknown trace channel \"" + word + "\"; known channels: "
                              + knownNames() + ", all, none";
            return false;
        }
    }
    if (out) *out = m;
    return true;
}

std::string TraceMask::filterRules() const
{
    // Every channel gets rules, on or off — setFilterRules replaces the whole
    // rule set, so a partial string would reset unrelated categories to Qt's
    // defaults. Only info/debug are governed; warning/critical always pass.
    std::string rules;
    for (std::size_t i = 0; i < kChannelCount; ++i) {
        const bool on = has(static_cast<TraceChannel>(i));
        const char *val = on ? "true" : "false";
        for (const char *p : defs()[i].prefixes) {
            const std::string pre(p);
            rules += pre + ".info="   + val + "\n";
            rules += pre + ".debug="  + val + "\n";
            rules += pre + ".*.info=" + val + "\n";
            rules += pre + ".*.debug=" + val + "\n";
        }
    }
    return rules;
}

std::string TraceMask::toSpec() const
{
    if (!any()) return "none";
    if (m_bits == TraceMask::all().bits()) return "all";
    std::string s;
    for (std::size_t i = 0; i < kChannelCount; ++i) {
        if (has(static_cast<TraceChannel>(i))) {
            if (!s.empty()) s += ",";
            s += defs()[i].name;
        }
    }
    return s;
}

LaunchTrace parseLaunch(const char *envTrace, const char *envMinutes,
                        const std::vector<std::string> &args, TraceMask persisted)
{
    LaunchTrace lt;
    lt.mask = persisted;

    if (envMinutes && *envMinutes) {
        try { lt.minutes = std::stoi(envMinutes); } catch (...) { /* ignore junk */ }
    }
    if (envTrace && *envTrace) {
        TraceMask m;
        std::string err;
        if (TraceMask::parse(envTrace, lt.mask, &m, &err)) lt.mask = m;
        else lt.error = "SNAPCLONE_TRACE: " + err;
    }

    // CLI last so a typed flag beats a stale shell profile.
    for (std::size_t i = 0; i < args.size(); ++i) {
        const std::string &a = args[i];
        auto valueOf = [&](const std::string &flag) -> const char * {
            if (a.rfind(flag + "=", 0) == 0) return a.c_str() + flag.size() + 1;
            if (a == flag && i + 1 < args.size()) return args[++i].c_str();
            return nullptr;
        };
        if (a == "--trace-help") { lt.showHelp = true; continue; }
        if (a == "--trace-all")  { lt.mask = TraceMask::all(); continue; }
        if (a == "--no-trace")   { lt.mask = TraceMask::none(); lt.explicitOff = true; continue; }
        if (const char *v = valueOf("--trace-minutes")) {
            try { lt.minutes = std::stoi(v); } catch (...) {}
            continue;
        }
        if (const char *v = valueOf("--trace")) {
            TraceMask m;
            std::string err;
            if (TraceMask::parse(v, lt.mask, &m, &err)) lt.mask = m;
            else lt.error = "--trace: " + err;
            continue;
        }
    }
    return lt;
}

std::string helpText()
{
    std::string s =
        "SnapClone diagnostic tracing — off by default; every channel below is\n"
        "silent until you ask for it.\n\n"
        "  Channels:\n";
    for (std::size_t i = 0; i < kChannelCount; ++i) {
        std::string n = defs()[i].name;
        n.resize(9, ' ');
        s += "    " + n + defs()[i].help + "\n";
    }
    s += "\n"
         "  Turn on at launch (works on an installed build, no rebuild needed):\n"
         "    SnapClone --trace=input,layout      one or more channels\n"
         "    SnapClone --trace-all               everything\n"
         "    SnapClone --trace=+net              add to what's already saved\n"
         "    SnapClone --no-trace                force off, ignore saved state\n"
         "    SnapClone --trace-minutes=60        auto-off after N minutes (0 = never)\n"
         "    SNAPCLONE_TRACE=input SnapClone     same, via the environment\n\n"
         "  Turn on while it's running:\n"
         "    Settings -> Debug -> Diagnostic tracing, one switch per channel.\n"
         "    The choice is saved, so a channel you enable now is still on after a\n"
         "    restart — which is how you catch something that breaks during startup.\n\n"
         "  Tracing auto-disables after " + std::to_string(kDefaultTraceMinutes) +
         " minutes by default so a field build never\n"
         "  quietly fills the disk. Warnings and errors are never suppressed.\n";
    return s;
}

bool traceEnabled(TraceChannel c) noexcept
{
    return (g_live.load(std::memory_order_relaxed) >> static_cast<int>(c)) & 1u;
}

void setLiveMask(TraceMask m) noexcept
{
    g_live.store(m.bits(), std::memory_order_relaxed);
}

TraceMask liveMask() noexcept
{
    return TraceMask{g_live.load(std::memory_order_relaxed)};
}

bool traceExpired(std::int64_t nowMs, std::int64_t deadlineMs) noexcept
{
    return deadlineMs > 0 && nowMs >= deadlineMs;
}

} // namespace SnapClone::Debug
