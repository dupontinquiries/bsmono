#pragma once
// ─── ui/CoalescingDebouncer.hpp ───────────────────────────────────────────────
//
// Debounce/coalesce accumulator for UI transitions. C++ MIRROR of the tested
// Rust crate `rust/snapclone-debounce` — the Rust crate is the source of truth
// and carries the unit tests (coalesce-to-latest, leading+trailing edges,
// max-wait cap, redundant-settle suppression, flush). This header reproduces the
// SAME state machine so the UI layer can use it without an FFI link for a few
// dozen lines of timing arithmetic. Keep the two in lockstep: change a rule
// here, change it (and its test) in the Rust crate.
//
// # What it's for (the layer separation)
//
// A swipe transition triggers BOTH instant UI sync (tab highlight) AND
// expensive/consequential work (network priorities, usage stats, celebrations).
// Firing the expensive work for every page a fast swipe blows past spams the
// system and wastes battery. This splits the raw transition stream into two
// edges: a LEADING edge (fire responsive work now) and a SETTLED edge (fire the
// expensive work ONCE, for the page actually landed on, after motion quiets).
//
// Driving: construct with a config, call push(value, nowMs) on each transition
// and tick(nowMs) from one repeating timer. Times are caller-supplied monotonic
// milliseconds (QElapsedTimer / steady clock) so it needs no clock of its own.
// ─────────────────────────────────────────────────────────────────────────────

#include <cstdint>
#include <optional>
#include <vector>

namespace SnapClone::UI {

/// The kind of edge emitted.
enum class DebounceEdgeKind { Leading, Settled };

template <typename T>
struct DebounceEdge {
    DebounceEdgeKind kind;
    T value;
};

struct DebounceConfig {
    uint64_t quietMs   = 180;   ///< quiet period before a Settled edge fires
    uint64_t maxWaitMs = 600;   ///< hard cap during continuous motion (0 = none)
    bool     leading   = true;  ///< emit a Leading edge on first change after quiet

    /// Page-transition default: instant leading edge, settle 180ms after the
    /// last swipe, never wait more than 600ms of continuous swiping.
    static DebounceConfig pageTransition() { return DebounceConfig{180, 600, true}; }
    /// Pure trailing debounce (no leading) for save-style consumers.
    static DebounceConfig trailingOnly(uint64_t quietMs) {
        return DebounceConfig{quietMs, 0, false};
    }
};

/// Leading+trailing debounce that coalesces to the latest value. Mirror of
/// snapclone_debounce::CoalescingDebouncer.
template <typename T>
class CoalescingDebouncer {
public:
    explicit CoalescingDebouncer(DebounceConfig cfg) : m_cfg(cfg) {}

    bool isActive() const { return m_active; }
    const std::optional<T> &pending() const { return m_pending; }

    /// Record a transition to `value` at `nowMs`. Returns a Leading edge if this
    /// opens a new burst and leading is enabled; otherwise nullopt (coalesced).
    std::optional<DebounceEdge<T>> push(const T &value, uint64_t nowMs) {
        const bool opening = !m_active;
        m_pending = value;
        m_lastPushMs = nowMs;
        if (opening) {
            m_burstStartMs = nowMs;
            m_haveBurstStart = true;
            m_active = true;
            if (m_cfg.leading) {
                return DebounceEdge<T>{DebounceEdgeKind::Leading, value};
            }
        }
        return std::nullopt;
    }

    /// Advance time; returns any edges that became due (at most one Settled).
    std::vector<DebounceEdge<T>> tick(uint64_t nowMs) {
        std::vector<DebounceEdge<T>> edges;
        if (!m_active) return edges;
        const bool quietElapsed =
            (nowMs >= m_lastPushMs) && (nowMs - m_lastPushMs >= m_cfg.quietMs);
        const bool capped =
            m_cfg.maxWaitMs > 0 && m_haveBurstStart && (nowMs >= m_burstStartMs)
            && (nowMs - m_burstStartMs >= m_cfg.maxWaitMs);
        if (quietElapsed || capped) {
            if (m_pending.has_value()) {
                const bool redundant =
                    m_lastSettled.has_value() && (*m_lastSettled == *m_pending);
                T v = *m_pending;
                m_pending.reset();
                m_lastSettled = v;
                if (!redundant) {
                    edges.push_back(DebounceEdge<T>{DebounceEdgeKind::Settled, v});
                }
            }
            m_active = false;
            m_haveBurstStart = false;
        }
        return edges;
    }

    /// Force an immediate settle (shutdown/background). Returns the Settled edge
    /// if a non-redundant value was pending.
    std::optional<DebounceEdge<T>> flush() {
        if (!m_active) return std::nullopt;
        m_active = false;
        m_haveBurstStart = false;
        if (!m_pending.has_value()) return std::nullopt;
        const bool redundant =
            m_lastSettled.has_value() && (*m_lastSettled == *m_pending);
        T v = *m_pending;
        m_pending.reset();
        m_lastSettled = v;
        if (redundant) return std::nullopt;
        return DebounceEdge<T>{DebounceEdgeKind::Settled, v};
    }

    /// Drop in-flight state without emitting. Keeps lastSettled so a re-show on
    /// the same value doesn't re-fire consequential work.
    void reset() {
        m_pending.reset();
        m_active = false;
        m_haveBurstStart = false;
    }

private:
    DebounceConfig    m_cfg;
    std::optional<T>  m_pending;
    std::optional<T>  m_lastSettled;
    uint64_t          m_lastPushMs    = 0;
    uint64_t          m_burstStartMs  = 0;
    bool              m_haveBurstStart = false;
    bool              m_active        = false;
};

} // namespace SnapClone::UI
