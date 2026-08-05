"""Deadline instants — what the engine registers, not what a calendar suggests.

`add_werktage` answers "which date", these answer "which moment". The two are
different, and only the moment is the obligation: a Werktage Frist expires at
**17:00 Europe/Berlin** on the due Werktag.

A monitor in this repo once approximated Werktage as calendar days and reported
breaches that had not happened. These bindings exist so a test can assert the
platform's own instant rather than re-deriving it — and so the difference is
visible in the test itself.
"""

import pytest

from makotest import (
    add_hours,
    add_werktage,
    aperak_gas_folgeprozess_due_at,
    aperak_gas_initialprozess_due_at,
    aperak_strom_due_at,
    assert_deadline_is,
    contrl_due_at,
    deadline_at_werktage,
    wim_antwort_frist_werktage,
)


def test_a_werktage_deadline_expires_at_1700_berlin_not_midnight():
    """The reason `add_werktage` is not enough on its own."""
    received = "2026-03-02T09:00:00Z"          # Monday
    assert add_werktage("2026-03-02", 3) == "2026-03-05"   # a date
    due = deadline_at_werktage(received, 3)                # an instant
    assert due.startswith("2026-03-05T17:00:00")
    # March is still CET, so the offset is +01:00 — not UTC.
    assert due.endswith("+01:00")


def test_the_offset_follows_the_cet_cest_transition():
    """Rendering the deadline in UTC would hide the hour that matters."""
    winter = deadline_at_werktage("2026-01-12T09:00:00Z", 1)
    summer = deadline_at_werktage("2026-07-13T09:00:00Z", 1)
    assert winter.endswith("+01:00")   # CET
    assert summer.endswith("+02:00")   # CEST
    assert "T17:00:00" in winter and "T17:00:00" in summer


def test_weekends_and_holidays_push_the_deadline_out():
    # Friday + 1 WT lands on Monday, not Saturday.
    assert deadline_at_werktage("2026-03-06T09:00:00Z", 1).startswith("2026-03-09")
    # The sharpest case in the calendar: **one** Werktag from Wednesday 30.12.
    # expires on Monday 04.01. — five calendar days later. 31.12. and 01.01. are
    # non-Werktage under the BDEW calendar and 02./03.01. is a weekend.
    #
    # Any "n Werktage ≈ n + 2 calendar days" rule reports this one as breached
    # three days early.
    assert (
        deadline_at_werktage("2026-12-30T09:00:00Z", 1) == "2027-01-04T17:00:00+01:00"
    )


# ── WiM MSB-Wechsel: four processes, four windows ────────────────────────────


@pytest.mark.parametrize(
    ("pid", "werktage"),
    [(55039, 3), (55042, 5), (55051, 7), (55168, 1)],
)
def test_the_wim_msb_wechsel_frist_is_per_pid(pid, werktage):
    """BK6-22-024 WiM Teil 1 Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2.

    Sizing all four the same escalates the Abmeldung (7 WT) early and hides a
    missed Verpflichtungsanfrage (1 WT) for days.
    """
    assert wim_antwort_frist_werktage(pid) == werktage


def test_a_non_msb_wechsel_pid_has_no_antwortfrist():
    assert wim_antwort_frist_werktage(55001) is None


def test_the_four_windows_are_not_one_value():
    windows = {wim_antwort_frist_werktage(p) for p in (55039, 55042, 55051, 55168)}
    assert windows == {1, 3, 5, 7}


# ── The other clocks ─────────────────────────────────────────────────────────


def test_aperak_strom_is_45_minutes_on_a_weekday_not_werktage():
    """The APERAK acknowledgement and the business answer are different clocks.

    Conflating them is the classic WiM error — 45 minutes versus days.
    """
    due = aperak_strom_due_at("2026-03-02T09:00:00Z")
    assert due.startswith("2026-03-02T")
    # Far shorter than even the shortest business window.
    assert due < deadline_at_werktage("2026-03-02T09:00:00Z", 1)


def test_gas_aperak_windows_differ_by_process_kind():
    received = "2026-03-02T09:00:00Z"
    folge = aperak_gas_folgeprozess_due_at(received)
    initial = aperak_gas_initialprozess_due_at(received)
    assert folge < initial, "the Folgeprozess window is the tighter of the two"


def test_contrl_is_six_wall_clock_hours():
    assert contrl_due_at("2026-03-02T09:00:00Z") == add_hours("2026-03-02T09:00:00Z", 6)


def test_add_hours_is_wall_clock_and_ignores_the_calendar():
    """GPKE's 24 hours run through a weekend; Werktage do not."""
    saturday_deadline = add_hours("2026-03-07T09:00:00Z", 24)
    assert saturday_deadline.startswith("2026-03-08")   # Sunday


# ── The assertion helper ─────────────────────────────────────────────────────


def test_assert_deadline_is_accepts_the_engine_instant():
    received = "2026-03-02T09:00:00Z"
    assert_deadline_is(deadline_at_werktage(received, 7), received=received, werktage=7)


def test_assert_deadline_is_rejects_a_date_only_comparison():
    """Midnight on the right day is still the wrong instant."""
    received = "2026-03-02T09:00:00Z"
    with pytest.raises(AssertionError) as excinfo:
        assert_deadline_is("2026-03-11T00:00:00Z", received=received, werktage=7)
    message = str(excinfo.value)
    assert "expected:" in message and "actual:" in message
    assert "17:00" in message, "the failure must show the real cutoff"


def test_a_bad_datetime_is_rejected_with_a_useful_message():
    with pytest.raises(ValueError, match="RFC 3339"):
        deadline_at_werktage("2026-03-02", 3)
