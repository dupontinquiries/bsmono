"""
CLI runner for the swipe sim harness.

Usage:
    python -m sim.run                    # run all scenarios
    python -m sim.run --trace            # also print decision steps
    python -m sim.run --scenario NAME    # run only the named scenario
    python -m sim.run --list             # list scenario names
    python -m sim.run --verbose          # detailed output even on PASS

Exit code is 0 if all scenarios PASS, 1 otherwise.

Run from the project root (one level above `sim/`):
    cd /path/to/rusty-board
    python -m sim.run
"""

from __future__ import annotations
import argparse
import sys
from typing import Sequence

from .dispatch import drag_end, DragEndOutcome
from .scenarios import SCENARIOS, Scenario


# ANSI colors — disabled if stdout isn't a TTY (e.g., piped to a file).
def _supports_color() -> bool:
    return sys.stdout.isatty()


COLOR_GREEN = "\033[32m" if _supports_color() else ""
COLOR_RED = "\033[31m" if _supports_color() else ""
COLOR_YELLOW = "\033[33m" if _supports_color() else ""
COLOR_DIM = "\033[2m" if _supports_color() else ""
COLOR_RESET = "\033[0m" if _supports_color() else ""


def _pass(label: str) -> str:
    return f"{COLOR_GREEN}[PASS]{COLOR_RESET} {label}"


def _fail(label: str) -> str:
    return f"{COLOR_RED}[FAIL]{COLOR_RESET} {label}"


def _dim(text: str) -> str:
    return f"{COLOR_DIM}{text}{COLOR_RESET}"


def run_scenario(
    scenario: Scenario,
    verbose: bool = False,
    trace: bool = False,
) -> bool:
    """Run one scenario. Returns True on pass, False on fail."""
    actual = drag_end(scenario.path, scenario.env)

    outcome_ok = actual.outcome == scenario.expected_outcome
    classify_ok = True
    direction_ok = True

    if scenario.expected_classify_kind is not None:
        if actual.cardinal_result is None:
            classify_ok = False
        elif actual.cardinal_result.kind != scenario.expected_classify_kind:
            classify_ok = False

    if scenario.expected_direction is not None:
        if (actual.cardinal_result is None
                or actual.cardinal_result.direction != scenario.expected_direction):
            direction_ok = False

    passed = outcome_ok and classify_ok and direction_ok

    if passed:
        print(_pass(scenario.name))
        if verbose:
            _print_detail(scenario, actual, trace_only_on_fail=False)
        elif trace:
            _print_trace(actual)
        return True

    # Failure — show what went wrong.
    print(_fail(scenario.name))
    _print_detail(scenario, actual, trace_only_on_fail=True)
    return False


def _print_detail(
    scenario: Scenario,
    actual: DragEndOutcome,
    trace_only_on_fail: bool,
) -> None:
    print(_dim(f"       description: {scenario.description}"))
    print(_dim(f"       expected:    {scenario.expected_outcome.value}"))
    if scenario.expected_classify_kind:
        print(_dim(
            f"                    classify={scenario.expected_classify_kind}"
            + (f", direction={scenario.expected_direction.value}"
               if scenario.expected_direction else "")
        ))
    print(_dim(f"       actual:      {actual.outcome.value}"))
    if actual.cardinal_result:
        print(_dim(f"                    {actual.cardinal_result!r}"))
        print(_dim(f"                    {actual.cardinal_result.reason}"))
    if actual.minimize_verdict:
        verdict = actual.minimize_verdict
        marker = "DETECTED" if verdict.detected else "rejected"
        print(_dim(f"       minimize:    {marker} — {verdict.reason}"))
    _print_trace(actual)


def _print_trace(actual: DragEndOutcome) -> None:
    print(_dim("       trace:"))
    for step in actual.trace:
        print(_dim(f"         · {step}"))


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--scenario", "-s",
        help="run only the named scenario",
    )
    parser.add_argument(
        "--trace", "-t",
        action="store_true",
        help="print decision-tree trace on PASS as well as FAIL",
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="detailed output even on PASS",
    )
    parser.add_argument(
        "--list", "-l",
        action="store_true",
        help="list available scenarios and exit",
    )
    args = parser.parse_args(argv)

    if args.list:
        for fn in SCENARIOS:
            scenario = fn()
            print(f"{scenario.name:50s} {COLOR_DIM}{scenario.description}{COLOR_RESET}")
        return 0

    scenarios: list[Scenario] = []
    for fn in SCENARIOS:
        scenario = fn()
        if args.scenario and scenario.name != args.scenario:
            continue
        scenarios.append(scenario)

    if args.scenario and not scenarios:
        print(f"{COLOR_RED}No scenario named: {args.scenario}{COLOR_RESET}",
              file=sys.stderr)
        print("Available:", file=sys.stderr)
        for fn in SCENARIOS:
            print(f"  {fn().name}", file=sys.stderr)
        return 2

    passed = 0
    failed = 0
    for scenario in scenarios:
        ok = run_scenario(scenario, verbose=args.verbose, trace=args.trace)
        if ok:
            passed += 1
        else:
            failed += 1

    total = passed + failed
    print()
    if failed == 0:
        print(f"{COLOR_GREEN}All {total} scenarios passed.{COLOR_RESET}")
    else:
        print(f"{COLOR_RED}{failed} of {total} scenarios FAILED.{COLOR_RESET}")
        print(f"{COLOR_GREEN}{passed} passed.{COLOR_RESET}")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
