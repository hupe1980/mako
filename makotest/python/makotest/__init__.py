"""makotest — test & simulation toolkit for German market-communication platforms.

Generates regulator-conformant inputs (EDIFACT, EPEX price curves, meter data),
simulates the external counterparties a MaKo platform talks to, and asserts on
the result.

Not mako-specific: everything it drives is a public wire contract (EDIFACT over
AS4, REST, CloudEvents), so it can exercise any MaKo implementation.

The wire-format primitives come from the same Rust crates the platform runs, so
`makotest` and the system under test can never disagree about what "valid" means::

    >>> from makotest import malo_from_base, add_werktage
    >>> malo_from_base("5123869678")
    '51238696780'
    >>> add_werktage("2026-12-24", 2)   # skips Christmas + the weekend
    '2026-12-29'

Because validation runs the platform's own AHB engine, `makotest` proves process
and integration behaviour — it is not an *independent* check of mako's format
conformance. The BDEW reference examples remain the authority for that.
"""

from __future__ import annotations

# Assertion helpers must be rewritten by pytest to report the actual mismatch
# instead of a bare `assert False`. Registration has to happen before the module
# is first imported, so it belongs here — but pytest stays an optional
# dependency: the simulators and generators are used from demos and notebooks
# too, and a hard pytest import would drag the framework into all of them.
try:  # pragma: no cover - trivial import guard
    import pytest as _pytest

    _pytest.register_assert_rewrite("makotest.assertions")
except ImportError:  # pytest not installed — core still fully usable
    pass

from ._native import (
    Finding,
    UtilmdTransaction,
    ValidationReport,
    add_werktage,
    build_interchange,
    build_mscons,
    build_utilmd,
    is_werktag,
    malo_check_digit,
    malo_from_base,
    malo_is_valid,
    melo_is_valid,
    next_werktag,
    validate_edifact,
)

__all__ = [
    "Finding",
    "UtilmdTransaction",
    "ValidationReport",
    "add_werktage",
    "bo4e_generation",
    "build_interchange",
    "build_mscons",
    "build_utilmd",
    "is_werktag",
    "malo_check_digit",
    "malo_from_base",
    "malo_is_valid",
    "melo_is_valid",
    "next_werktag",
    "validate_edifact",
]

try:  # pragma: no cover - packaging metadata lookup
    from importlib.metadata import version as _pkg_version

    #: Tracks `workspace.package.version` via Cargo.toml — never hardcoded here,
    #: so the wheel and the crates it binds cannot report different versions.
    __version__ = _pkg_version("makotest")
except Exception:  # not installed (e.g. running from a source checkout)
    __version__ = "0.0.0+unknown"

#: The BO4E release this toolkit's objects are generated from.
#:
#: mako's Rust side generates from the same release via ``rubo4e``. Testing
#: v202607 objects against a platform on a different generation produces
#: meaningless passes, so :func:`assert_bo4e_generation_matches` compares this
#: against whatever the platform advertises.
BO4E_GENERATION = "202607"


def bo4e_generation() -> str:
    """The BO4E release `makotest` builds business objects from."""
    return BO4E_GENERATION
