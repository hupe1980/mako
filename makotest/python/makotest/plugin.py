"""The pytest plugin — registered through the standard `pytest11` entry point.

This module is the *only* part of `makotest` that imports pytest. Everything it
exposes is a thin wrapper over objects that work standalone, so a demo script
and a CI test drive the same code path.
"""

from __future__ import annotations

import datetime as _dt

import pytest

from .generators import EpexSim
from .simulators import BikoSim, ImsysSim, MarktpartnerSim

__all__ = [
    "biko_sim",
    "epex_sim",
    "frozen_clock",
    "imsys_sim",
    "nb_sim",
    "pytest_addoption",
    "pytest_configure",
]


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup("makotest", "German market-communication testing")
    group.addoption(
        "--mako-endpoint",
        action="store",
        default=None,
        metavar="URL",
        help=(
            "Run against an already-running deployment instead of starting "
            "containers. Useful against a staging environment."
        ),
    )
    group.addoption(
        "--makotest-seed",
        action="store",
        type=int,
        default=0,
        metavar="N",
        help="Seed for every generator, so a failing run can be reproduced exactly.",
    )


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line(
        "markers",
        "regulatory(spec): the Festlegung or § this test pins, e.g. "
        '@pytest.mark.regulatory("GPKE Teil 2 SD Lieferbeginn")',
    )
    config.addinivalue_line(
        "markers",
        "requires_docker: needs a Docker daemon (testcontainers-backed fixtures)",
    )


@pytest.fixture(scope="session")
def makotest_seed(request: pytest.FixtureRequest) -> int:
    """The generator seed for this session (`--makotest-seed`)."""
    return int(request.config.getoption("--makotest-seed"))


@pytest.fixture(scope="session")
def mako_endpoint(request: pytest.FixtureRequest) -> str | None:
    """Base URL of an already-running deployment, or `None` to self-host."""
    return request.config.getoption("--mako-endpoint")


@pytest.fixture
def epex_sim(makotest_seed: int) -> EpexSim:
    """A seeded EPEX day-ahead curve generator."""
    return EpexSim(seed=makotest_seed)


class FrozenClock:
    """A clock the test moves deliberately.

    Regulated processes are deadline-driven, so time is an input, never ambient:
    a test that passes on Tuesday and fails on a Feiertag is not a test of the
    system. `advance_werktage` delegates to the Rust BDEW calendar, so the test
    and the platform agree on what a Werktag is.
    """

    def __init__(self, at: str) -> None:
        self._now = _dt.datetime.fromisoformat(at)

    @property
    def now(self) -> _dt.datetime:
        return self._now

    @property
    def date(self) -> str:
        """Current date as ISO 8601 — the form the native helpers take."""
        return self._now.date().isoformat()

    def advance(self, **timedelta_kwargs: float) -> "FrozenClock":
        self._now += _dt.timedelta(**timedelta_kwargs)
        return self

    def advance_werktage(self, n: int) -> "FrozenClock":
        """Advance `n` Werktage under the BDEW MaKo calendar."""
        from ._native import add_werktage

        new_date = _dt.date.fromisoformat(add_werktage(self.date, n))
        self._now = _dt.datetime.combine(new_date, self._now.timetz())
        return self

    def __repr__(self) -> str:
        return f"FrozenClock({self._now.isoformat()})"


@pytest.fixture
def frozen_clock() -> FrozenClock:
    """A clock frozen at a fixed, Werktag-aware instant.

    Defaults to a Tuesday so `advance_werktage(1)` does not accidentally cross a
    weekend in the common case — tests that care about weekend behaviour should
    set their own instant.
    """
    return FrozenClock("2026-11-03T09:00:00+01:00")


# ── Counterparty simulators ───────────────────────────────────────────────────
#
# Each fixture is function-scoped: a simulator accumulates the exchanges it
# handled, and sharing that across tests would make one test's assertions
# depend on another's traffic.


@pytest.fixture
def nb_sim() -> MarktpartnerSim:
    """A Netzbetreiber counterparty with no answers bound.

    Unconfigured means silent, which is the safe default: a test that forgets
    to bind an answer exercises the Frist path rather than silently passing on
    a response it never asked for.
    """
    return MarktpartnerSim(mp_id="9900357000004", rolle="NB")


@pytest.fixture
def biko_sim() -> BikoSim:
    """A Bilanzkoordinator that accepts submissions until told otherwise."""
    return BikoSim(mp_id="9979999000007")


@pytest.fixture
def imsys_sim() -> ImsysSim:
    """A Smart-Meter-Gateway with a valid certificate and TAF-7."""
    return ImsysSim(
        melo_id="DE0006819497000000000000000001234",
        taf="TAF-7",
        today="2026-11-03",
    )
