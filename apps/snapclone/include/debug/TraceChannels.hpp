#pragma once
// ─── debug/TraceChannels.hpp ─────────────────────────────────────────────────
//
// The trace-channel policy core: which diagnostic logging categories exist, how
// they group into user-facing CHANNELS, how a channel spec ("input,layout",
// "all", "+net") parses, and what QLoggingCategory filter-rule text a given set
// of enabled channels produces.
//
// WHY THIS EXISTS
//   Per-event diagnostics like
//     snapclone.ui.swipenav [I] [WHEEL] v=-20 h=0 subTabbed=1 atVLimit=0
//   were invaluable while the swipe/wheel handoff was being built and are pure
//   noise now that it works: they fire on every wheel notch, cost a snprintf
//   plus a log write each, and bloat the log file so the ONE line that matters
//   during a real bug report is buried. But deleting them is also wrong — the
//   next person debugging a touch-handoff issue needs exactly those lines, and
//   they will be running an INSTALLED build with no compiler in reach.
//
//   So: keep the statements, make them cost ~nothing when off, and give three
//   independent ways to switch them on — settings toggle (mid-session), env
//   var, and CLI flag (both from launch, for a build already in the field).
//
// THE COST MODEL (what "wasteful" means, precisely)
//   * The expensive part of a disabled log line is not the write — Qt already
//     drops it — it is FORMATTING the arguments and building the QString
//     before Qt gets a chance to drop anything.
//   * SC_TRACE (see Trace.hpp) wraps the call in a relaxed atomic load of the
//     channel bit, so when the channel is off the arguments are never even
//     evaluated. Cost when off: one load + one predictable branch.
//   * SC_TRACE_HOT additionally compiles to nothing in production builds, for
//     paths where even that branch is unwelcome (per-frame paint/render).
//     Enable it in a field build with -DSNAPCLONE_HOT_TRACE=ON.
//
// This header is deliberately Qt-FREE so the policy is unit-testable with a
// plain compiler (tests/test_TraceChannels.cpp); the Qt glue that persists
// state, applies the rules, and drives expiry lives in TraceControl.
// ─────────────────────────────────────────────────────────────────────────────

#include <cstdint>
#include <string>
#include <vector>

namespace SnapClone::Debug {

/// User-facing diagnostic channels. Each owns a disjoint set of logging-category
/// prefixes (see kChannelPrefixes); "disjoint" is enforced by a unit test so a
/// new category can't silently land in two channels with contradictory rules.
enum class TraceChannel : std::uint8_t {
    Input = 0,   ///< swipe/wheel/gesture classification and hand-off
    Layout,      ///< virtual keyboard, modal stack, dialog geometry
    Camera,      ///< capture pipeline, focus/VCM, camera registry
    Media,       ///< encode/record/transcode, viewer, thumbnails, export
    Net,         ///< sockets, NAT/UPnP, relays, gossip, routing
    Dht,         ///< DHT + identity/accounts resolution
    Storage,     ///< on-disk stores, inbox/outbox, databus, session
    Voice,       ///< voice capture/playback and group-voice coordination
    Ui,          ///< ambient visuals, conversation views, icons, wayfinder
    Power,       ///< power/sleep-state policy
    Count
};

inline constexpr std::size_t kChannelCount = static_cast<std::size_t>(TraceChannel::Count);

/// Stable lowercase name used in specs, settings keys, env vars and CLI flags.
[[nodiscard]] const char *name(TraceChannel c) noexcept;
/// One-line description shown by `--trace-help` (the field-debugging entry point).
[[nodiscard]] const char *help(TraceChannel c) noexcept;
/// Logging-category prefixes this channel owns. Each yields both an exact rule
/// and a `<prefix>.*` wildcard rule.
[[nodiscard]] std::vector<const char *> prefixes(TraceChannel c);

/// A set of enabled channels, plus the pure policy operations over it.
class TraceMask {
public:
    TraceMask() = default;
    explicit constexpr TraceMask(std::uint32_t bits) : m_bits(bits) {}

    [[nodiscard]] static constexpr TraceMask none() { return TraceMask{0}; }
    [[nodiscard]] static TraceMask all();

    [[nodiscard]] constexpr std::uint32_t bits() const { return m_bits; }
    [[nodiscard]] bool has(TraceChannel c) const { return (m_bits >> static_cast<int>(c)) & 1u; }
    void set(TraceChannel c, bool on);
    [[nodiscard]] bool any() const { return m_bits != 0; }

    /// Parse a spec against `base` (the current mask, for relative +/- forms).
    ///   "all" / "none" / "off"        — absolute
    ///   "input,layout"                — absolute set
    ///   "+net" / "-input"             — relative to `base`
    /// Separators: comma, space, semicolon. Case-insensitive. On an unknown
    /// name, returns std::nullopt-ish (false) and fills `error` with a message
    /// that LISTS the valid names — the person typing it is usually debugging
    /// an installed build and has no docs open.
    [[nodiscard]] static bool parse(const std::string &spec, TraceMask base,
                                    TraceMask *out, std::string *error);

    /// The complete QLoggingCategory filter-rule text for this mask.
    ///
    /// Emits rules for EVERY channel — enabled ones true, disabled ones false —
    /// because QLoggingCategory::setFilterRules REPLACES the entire rule set on
    /// each call. (The previous UiDebugFlags::apply() emitted rules only for the
    /// categories it knew about, so applying it silently reset every other
    /// category to Qt's defaults.) Warning and critical levels are never
    /// touched: real problems always surface, tracing only governs info/debug.
    [[nodiscard]] std::string filterRules() const;

    /// Comma-joined names of enabled channels ("none" when empty). Round-trips
    /// through parse(); used for settings persistence and the status line.
    [[nodiscard]] std::string toSpec() const;

private:
    std::uint32_t m_bits = 0;
};

/// Result of reading launch-time configuration.
struct LaunchTrace {
    TraceMask mask;             ///< channels to enable at start
    bool showHelp = false;      ///< --trace-help was passed: print and continue
    bool explicitOff = false;   ///< --no-trace: force off, ignore persisted state
    int minutes = 0;            ///< --trace-minutes=N (0 = use the default)
    std::string error;          ///< non-empty when a spec failed to parse
};

/// Parse launch configuration from an env value and an argv-style list.
///
/// Env:  SNAPCLONE_TRACE=input,layout   SNAPCLONE_TRACE=all   SNAPCLONE_TRACE_MINUTES=60
/// CLI:  --trace=input,layout  --trace input,layout  --trace-all  --no-trace
///       --trace-help  --trace-minutes=60
///
/// CLI wins over env (a person typing a flag is more current than a shell
/// profile). `persisted` is the settings-stored mask that both can override.
[[nodiscard]] LaunchTrace parseLaunch(const char *envTrace, const char *envMinutes,
                                      const std::vector<std::string> &args,
                                      TraceMask persisted);

/// Human-readable `--trace-help` text: every channel, its description, and the
/// three ways to turn it on. Printed to stdout at launch and shown verbatim in
/// Settings → Debug so the same words appear wherever someone looks.
[[nodiscard]] std::string helpText();

// ── the runtime gate (read by the SC_TRACE macros) ───────────────────────────

/// Process-wide enabled-channel bits. Relaxed atomic: hot-path reads never lock,
/// and a stale read for one frame after a toggle is harmless.
[[nodiscard]] bool traceEnabled(TraceChannel c) noexcept;
/// Replace the live mask (called by TraceControl after settings/CLI/expiry).
void setLiveMask(TraceMask m) noexcept;
[[nodiscard]] TraceMask liveMask() noexcept;

/// Auto-expiry: tracing enabled from the UI or CLI turns itself off after this
/// many minutes so nobody leaves a field build writing diagnostics forever.
/// 0 disables expiry ("until I turn it off").
inline constexpr int kDefaultTraceMinutes = 30;

/// Pure expiry check driven by TraceControl's timer. Returns true when `nowMs`
/// has passed `deadlineMs` (deadline 0 = never expires).
[[nodiscard]] bool traceExpired(std::int64_t nowMs, std::int64_t deadlineMs) noexcept;

} // namespace SnapClone::Debug
