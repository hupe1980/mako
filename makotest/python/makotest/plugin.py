"""The pytest plugin — registered through the standard `pytest11` entry point.

This module is the *only* part of `makotest` that imports pytest. Everything it
exposes is a thin wrapper over objects that work standalone, so a demo script
and a CI test drive the same code path.

Two session-scoped inputs make a suite reproducible: `--makotest-seed` fixes
every generator, and `--makotest-on` fixes the BDEW format version every
message is built and validated against. Both default to a constant rather than
to "now", because a suite whose meaning changes at the next Formatumstellung is
not a suite.
"""

from __future__ import annotations

import datetime as _dt

import pytest

from ._native import (
    add_werktage,
    berlin_instant,
    format_versions,
    next_werktag,
    release_for,
)
from .generators import EpexGenerator, LastgangGenerator
from .simulators import BikoSim, ImsysSim, MarktpartnerSim

__all__ = [
    "biko_sim",
    "epex",
    "frozen_clock",
    "imsys_sim",
    "lastgang",
    "mako_endpoint",
    "makotest_on",
    "makotest_seed",
    "nb_sim",
    "pytest_addoption",
    "pytest_configure",
]

#: Reference date every fixture builds and validates against by default.
#:
#: A published format version, deliberately not "today": a message built on one
#: FV and validated on another produces findings that describe the mismatch
#: rather than the message, and a default that moves would make the same test
#: mean different things in different months.
#:
#: FV2026-04-01 is the earliest version on which **every** message type this
#: toolkit builds is active — CONTRL only enters the compiled set on
#: FV2026-01-01, so a simulator dated earlier cannot acknowledge anything.
DEFAULT_REFERENCE_DATE = "2026-04-01"

#: Marktpartner-IDs used by the fixtures. Each carries a **valid check digit**
#: — an invented one is exactly the defect this toolkit exists to catch, and a
#: fixture is the first place a reader copies from.
NB_ID = "9900357000003"  # BDEW-Codenummer, §8.1 check digit
LF_ID = "4012345000023"  # GS1 GLN, EAN-13 check digit
BIKO_ID = "9979999000002"  # BDEW-Codenummer, §8.1 check digit

#: `(message_type, sparte)` every fixture in this plugin can be asked to build.
#: `makotest_on` resolves each on the session date, so a reference date with a
#: gap in one of them is refused up front rather than surfacing as a build
#: failure in whichever test happens to need that type.
FIXTURE_MESSAGE_TYPES: tuple[tuple[str, str | None], ...] = (
    ("UTILMD", "STROM"),
    ("UTILMD", "GAS"),
    ("MSCONS", None),
    ("APERAK", None),
    ("CONTRL", None),
)

#: A 33-character Messlokations-ID for the Smart-Meter-Gateway fixture.
IMSYS_MELO = "DE0006819497000000000000000001234"


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
    group.addoption(
        "--makotest-on",
        action="store",
        default=DEFAULT_REFERENCE_DATE,
        metavar="ISO_DATE",
        help=(
            "Reference date selecting the BDEW format version every fixture "
            "builds and validates against. Re-run a suite on a future date to "
            "see what the next Formatumstellung breaks."
        ),
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
    _register_hypothesis_profile()


def _register_hypothesis_profile() -> None:
    """Register a `makotest` Hypothesis profile suited to this domain.

    Select it with `--hypothesis-profile=makotest`; registering does not activate
    it, so a suite with its own profile keeps it.

    Two settings, both about the same thing — a strategy here draws through the
    Rust core, and building or validating an interchange is orders of magnitude
    slower than drawing an integer:

    * `deadline=None`. Hypothesis' 200 ms per-example deadline is written for
      pure functions. A property that builds and validates EDIFACT exceeds it on
      a loaded machine and fails as `DeadlineExceeded`, which reads as a defect
      in the system under test and is not one.
    * `derandomize=True`. Draws come from the test's own identity rather than
      from entropy, so a CI run is reproducible from its command line.

    Hypothesis keeps its own seed knob (`--hypothesis-seed`); `--makotest-seed`
    seeds this toolkit's generators and deliberately does not reach into it.
    """
    try:
        from hypothesis import settings
    except ImportError:  # pragma: no cover - the extra is optional
        return
    settings.register_profile(
        "makotest", settings(derandomize=True, deadline=None, print_blob=True)
    )


@pytest.fixture(scope="session")
def makotest_seed(request: pytest.FixtureRequest) -> int:
    """The generator seed for this session (`--makotest-seed`)."""
    return int(request.config.getoption("--makotest-seed"))


@pytest.fixture(scope="session")
def makotest_on(request: pytest.FixtureRequest) -> str:
    """The reference date for this session (`--makotest-on`).

    Fails loudly when it names a date no compiled profile covers: every message
    built on it would fall back to an explicit release or fail outright, and a
    suite that silently validated nothing is the failure mode worth preventing.

    Being inside the published range is not enough — a date can fall in a gap
    between two versions of a type. Every type the fixtures build is therefore
    resolved on that date, and a missing one is named rather than discovered
    later as a build failure in an unrelated test.
    """
    on = str(request.config.getoption("--makotest-on"))
    try:
        _dt.date.fromisoformat(on)
    except ValueError as exc:
        raise pytest.UsageError(f"--makotest-on={on} is not an ISO 8601 date") from exc

    published = format_versions()
    earliest = min(v.removeprefix("FV") for v in published) if published else None
    if earliest is not None and on < earliest:
        raise pytest.UsageError(
            f"--makotest-on={on} predates every compiled format version "
            f"(earliest {earliest}). Nothing would validate against real rules."
        )
    absent = [
        message_type
        for message_type, sparte in FIXTURE_MESSAGE_TYPES
        if release_for(message_type, on, sparte) is None
    ]
    if absent:
        raise pytest.UsageError(
            f"--makotest-on={on} has no active profile for {', '.join(absent)}. "
            f"The fixtures build all of {', '.join(t for t, _ in FIXTURE_MESSAGE_TYPES)}"
            f"; pick a date inside a published format version "
            f"(see makotest.format_versions())."
        )
    return on


@pytest.fixture(scope="session")
def mako_endpoint(request: pytest.FixtureRequest) -> str | None:
    """Base URL of an already-running deployment, or `None` to self-host."""
    return request.config.getoption("--mako-endpoint")


@pytest.fixture
def epex(makotest_seed: int) -> EpexGenerator:
    """A seeded EPEX day-ahead curve generator.

    Not a simulator: it answers no messages, it produces input. The
    counterparty fixtures below respond; this one does not.
    """
    return EpexGenerator(seed=makotest_seed)


@pytest.fixture
def lastgang(makotest_seed: int) -> LastgangGenerator:
    """A seeded consumption / feed-in curve generator.

    Synthetic shapes, not Standardlastprofile: this build carries no BDEW
    coefficient tables, and a generator naming itself `H0` while inventing them
    would make every settlement asserted against it look authoritative and be
    wrong.
    """
    return LastgangGenerator(seed=makotest_seed)


class FrozenClock:
    """A clock the test moves deliberately, in Europe/Berlin.

    Regulated processes are deadline-driven, so time is an input, never ambient:
    a test that passes on Tuesday and fails on a Feiertag is not a test of the
    system. `advance_werktage` delegates to the Rust BDEW calendar, so the test
    and the platform agree on what a Werktag is — including the Landesfeiertage
    and 24./31.12., where a naive Python implementation silently diverges.

    The clock is a **Berlin local time**, and its UTC offset is resolved from the
    platform's own timezone database on every move. A clock that carried the
    offset it was constructed with would report `09:00+01:00` after advancing
    into summer time — an instant an hour off the wall clock it claims, in the
    same direction for every deadline the test then asserts.
    """

    def __init__(self, at: str) -> None:
        """`at` is a Berlin wall-clock instant, `YYYY-MM-DDTHH:MM[:SS]`.

        A UTC offset in `at` is ignored rather than honoured — the offset is a
        property of the date, and accepting a contradictory one would let a test
        pin an instant the German calendar does not have.
        """
        parsed = _dt.datetime.fromisoformat(at)
        self._at(parsed.date(), parsed.time())

    def _at(self, date: _dt.date, clock: _dt.time) -> None:
        self._now = _dt.datetime.fromisoformat(
            berlin_instant(date.isoformat(), clock.strftime("%H:%M:%S"))
        )

    @property
    def now(self) -> _dt.datetime:
        return self._now

    @property
    def date(self) -> str:
        """Current date as ISO 8601 — the form the native helpers take."""
        return self._now.date().isoformat()

    @property
    def instant(self) -> str:
        """Current instant as RFC 3339 — the form the deadline helpers take."""
        return self._now.isoformat()

    def advance(self, **timedelta_kwargs: float) -> FrozenClock:
        """Advance by a wall-clock `timedelta`, re-resolving the Berlin offset.

        Advancing by 24 hours across the March transition lands on the same wall
        clock the following day, not an hour earlier: a German business day is a
        local day and a Frist is stated in local time.
        """
        moved = self._now + _dt.timedelta(**timedelta_kwargs)
        self._at(moved.date(), moved.time())
        return self

    def advance_werktage(self, n: int) -> FrozenClock:
        """Advance `n` Werktage under the BDEW MaKo calendar."""
        self._at(_dt.date.fromisoformat(add_werktage(self.date, n)), self._now.time())
        return self

    def __repr__(self) -> str:
        return f"FrozenClock({self._now.isoformat()})"


@pytest.fixture
def frozen_clock(makotest_on: str) -> FrozenClock:
    """A clock frozen at 09:00 Berlin on the first Werktag of the session date.

    Anchored to `--makotest-on` rather than to a date of its own, so the clock
    and the format version every message is built against describe the same day.
    Construct a `FrozenClock` directly when the test is about a specific calendar
    edge.
    """
    return FrozenClock(f"{next_werktag(makotest_on)}T09:00:00")


# ── Counterparty simulators ───────────────────────────────────────────────────
#
# Each fixture is function-scoped: a simulator accumulates the exchanges it
# handled, and sharing that across tests would make one test's assertions depend
# on another's traffic.


@pytest.fixture
def nb_sim(makotest_on: str) -> MarktpartnerSim:
    """A Netzbetreiber counterparty with no answers bound.

    Unconfigured, it acknowledges and sends no business answer — the safe
    default: a test that forgets to bind one exercises the Frist path rather
    than silently passing on a response it never asked for. Total silence, the
    acknowledgement included, is `.timeout()`.
    """
    return MarktpartnerSim(mp_id=NB_ID, rolle="NB", reference_date=makotest_on)


@pytest.fixture
def biko_sim(makotest_on: str) -> BikoSim:
    """A Bilanzkoordinator that accepts submissions until told otherwise."""
    return BikoSim(mp_id=BIKO_ID, reference_date=makotest_on)


@pytest.fixture
def imsys_sim(makotest_on: str) -> ImsysSim:
    """A Smart-Meter-Gateway with a valid certificate, running TAF-7.

    Its clock is the session reference date, so a test that asserts across a
    gateway delivery and a Frist is talking about one day, not two.
    """
    return ImsysSim(melo_id=IMSYS_MELO, taf="TAF-7", today=makotest_on)
