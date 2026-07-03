#pragma once

// ─── Core/Result.hpp ─────────────────────────────────────────────────────────
//
// Minimal C++20-compatible Result<T, E> type.
//
// std::expected is C++23. Until compilers ship it by default under C++20 mode,
// this thin wrapper provides the same API surface we actually use:
//
//   Result<T, E>            — success or failure
//   Result<T, E>(value)     — success
//   makeError<T, E>(err)    — failure
//   result.has_value()
//   result.value()          / result->  / *result
//   result.error()
//
// Once the project moves to C++23 this header can be deleted and every
// #include replaced with <expected> and std::expected / std::unexpected.
// ─────────────────────────────────────────────────────────────────────────────

#include <variant>
#include <stdexcept>

namespace SnapClone {

// ── Sentinel type to wrap errors (mirrors std::unexpected) ───────────────────
template <typename E>
struct Unexpected {
    E error;
    explicit Unexpected(E e) : error(std::move(e)) {}
};

template <typename E>
Unexpected<E> makeUnexpected(E e) { return Unexpected<E>(std::move(e)); }

// ── Result<T, E> ─────────────────────────────────────────────────────────────
template <typename T, typename E>
class Result {
public:
    // Construct from a value (success).
    Result(T val) : m_data(std::in_place_index<0>, std::move(val)) {}      // NOLINT(*-explicit*)

    // Construct from an Unexpected (failure).
    Result(Unexpected<E> u) : m_data(std::in_place_index<1>, std::move(u.error)) {} // NOLINT(*-explicit*)

    [[nodiscard]] bool has_value() const noexcept { return m_data.index() == 0; }
    explicit operator bool()       const noexcept { return has_value(); }

    [[nodiscard]] const T &value() const & {
        if (!has_value()) throw std::logic_error("Result::value() called on error");
        return std::get<0>(m_data);
    }
    [[nodiscard]] T &value() & {
        if (!has_value()) throw std::logic_error("Result::value() called on error");
        return std::get<0>(m_data);
    }

    [[nodiscard]] const T *operator->() const { return &value(); }
    [[nodiscard]] T       *operator->()       { return &value(); }
    [[nodiscard]] const T &operator*()  const { return value(); }
    [[nodiscard]] T       &operator*()        { return value(); }

    [[nodiscard]] const E &error() const {
        if (has_value()) throw std::logic_error("Result::error() called on value");
        return std::get<1>(m_data);
    }

private:
    std::variant<T, E> m_data;
};

} // namespace SnapClone
