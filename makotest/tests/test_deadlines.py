"""Answer Fristen — four shapes, one table, and no default.

`add_werktage` answers "which date". These answer "which moment", and the moment
is the obligation.

The trap this module exists to close: "a Werktage Frist expires at 17:00
Europe/Berlin" is true of the WiM MSB-Wechsel windows and of nothing else. A
GPKE answer window is a **clock time on the n-th Werktag after the
Übertragungstag** — or, for the Ersatz-/Grundversorgung and the LF-Zuordnung,
that clock time **on the ÜT itself** — and a GeLi Gas window runs to the **end**
of the n-th Werktag. Sizing any of them the same is wrong in both directions,
and the loose direction reports a lapsed Frist as still running.

The two GPKE shapes share a clock time and land a day apart, so the Werktag
count is what separates them.
"""

import pytest

from makotest import (
    add_hours,
    add_werktage,
    antwort_deadline,
    antwort_obligation,
    antwort_obligations,
    aperak_gas_folgeprozess_due_at,
    aperak_gas_initialprozess_due_at,
    aperak_strom_due_at,
    assert_deadline_is,
    assert_frist_met,
    contrl_due_at,
    deadline_at_werktage,
    end_of_werktag_after,
    next_werktag_at,
)

MONDAY = "2026-03-02T09:00:00Z"


class TestTheThreeShapes:
    def test_gpke_is_a_clock_time_on_the_nth_werktag(self):
        """BK6-24-174 Teil 2: „11:00 Uhr des 1. WT nach dem ÜT" — not 24 hours.

        A message arriving Friday afternoon is answerable until Monday morning;
        one arriving Tuesday evening has under sixteen hours. A flat 24 h is
        both too tight and too loose.
        """
        o = antwort_obligation(55001)
        assert o.shape == "werktag_at"
        assert o.clock_time == "11:00"
        assert o.werktage == 1
        assert antwort_deadline(55001, MONDAY) == "2026-03-03T11:00:00+01:00"
        assert antwort_deadline(55001, MONDAY) != add_hours(MONDAY, 24)

    def test_the_neuanlage_window_is_the_same_shape_with_a_larger_n(self):
        """GPKE Teil 2 § 2.2.2: „00:00 Uhr des 61. WT nach dem ÜT".

        `E_0608` Prüfschritte 110/590 give the NB 60 Werktage of daily
        re-identification before it may refuse a newly commissioned
        Marktlokation, so this is a clock time on a far-away Werktag — not a
        duration, and not a day.
        """
        o = antwort_obligation(55600)
        assert o.shape == "werktag_at"
        assert o.werktage == 61
        assert o.clock_time == "00:00"
        assert antwort_obligation(55601).werktage == 61

    def test_geli_gas_runs_to_the_end_of_the_nth_werktag(self):
        """GeLi Gas 3.0 Kap. 3.2.3: „bis zum Ablauf des 4. Werktages".

        Day-granular, so it expires at the end of the day and not at an
        end-of-business hour. Sizing it at 17:00 expires it seven hours early.
        """
        o = antwort_obligation(44001)
        assert o.shape == "end_of_werktag"
        assert o.werktage == 4
        due = antwort_deadline(44001, MONDAY)
        assert due == end_of_werktag_after(MONDAY, 4)
        assert due.startswith("2026-03-06T23:59:59")

    def test_wim_expires_at_the_1700_cutoff(self):
        """BK6-24-174 WiM Teil 1: „spätester ÜT ist der n. WT", 17:00 MaKo cut-off."""
        o = antwort_obligation(55039)
        assert o.shape == "werktage_at_cutoff"
        assert o.werktage == 3
        assert antwort_deadline(55039, MONDAY) == deadline_at_werktage(MONDAY, 3)
        assert antwort_deadline(55039, MONDAY).startswith("2026-03-05T17:00:00")

    def test_the_three_are_three_different_instants(self):
        instants = {antwort_deadline(pid, MONDAY) for pid in (55001, 44001, 55039)}
        assert len(instants) == 3


class TestTheTable:
    def test_every_published_family_is_named(self):
        """The set is pinned, not counted: a family appearing without a test
        of its own is a window nothing here checks the arithmetic of."""
        families = {o.family for o in antwort_obligations()}
        assert families == {"emob", "geli-gas", "gpke", "wim", "wim-gas"}

    def test_the_modell_2_windows_are_seven_and_three_werktage(self):
        """AWH „Zum Modell 2" V1.3: the Anmeldung answer is 7 WT (Kap. 2.1.2
        Nr. 4), the Beendigung der Zuordnung and the Abmeldung 3 (Nr. 3,
        Kap. 2.2.2 Nr. 2).

        Seven, not three: `E_0510` Prüfschritt 1 asks whether the LF refused
        *within its own window*, which cannot be known before the 6th Werktag.
        """
        anmeldung = antwort_obligation(55238)
        assert anmeldung.family == "emob"
        assert anmeldung.werktage == 7
        for pid in (55240, 55242):
            assert antwort_obligation(pid).werktage == 3

    def test_every_obligation_cites_its_fundstelle(self):
        """Every window is measured in Werktage; the wall-clock shapes add a time.

        So a consumer can format the window from the shape alone, without a
        fallback branch. `same_day_at` reports ``werktage == 0`` — the deadline
        falls on the Übertragungstag itself rather than a Werktag after it.
        """
        wall_clock = {"werktag_at", "same_day_at"}
        for o in antwort_obligations():
            assert o.source, f"{o.trigger_pid} has no citation"
            assert o.werktage is not None, f"{o.trigger_pid} names no Werktage"
            assert (o.clock_time is not None) == (o.shape in wall_clock), (
                f"{o.trigger_pid}: only the wall-clock shapes carry a clock time"
            )

    def test_the_wim_msb_wechsel_windows_are_not_one_value(self):
        """Four separate Use-Cases of WiM Teil 1, four different windows.

        Sizing all four the same escalates the Abmeldung (7 WT) early and hides
        a missed Verpflichtungsanfrage (1 WT) for days.
        """
        assert {
            antwort_obligation(pid).werktage for pid in (55039, 55042, 55051, 55168)
        } == {1, 3, 5, 7}

    def test_an_unquantified_pid_reports_unknown_not_unbounded(self):
        """44020's Frist is set per Netzbetreiber under Kap. 2.6, so it is absent."""
        assert antwort_obligation(44020) is None
        assert antwort_deadline(44020, MONDAY) is None

    def test_the_answer_pids_come_from_the_same_row(self):
        o = antwort_obligation(55001)
        assert (o.bestaetigung_pid, o.ablehnung_pid) == (55002, 55003)
        assert antwort_obligation(55077).ablehnung_pid == 55080, "55079 is unassigned"


class TestTheOtherClocks:
    def test_aperak_strom_is_45_minutes_not_werktage(self):
        """The acknowledgement and the business answer are different clocks.

        Conflating them is the classic WiM error — 45 minutes versus days.
        """
        due = aperak_strom_due_at(MONDAY)
        assert due.startswith("2026-03-02T")
        assert due < antwort_deadline(55001, MONDAY)

    def test_gas_aperak_windows_differ_by_process_kind(self):
        assert aperak_gas_folgeprozess_due_at(MONDAY) < aperak_gas_initialprozess_due_at(
            MONDAY
        )

    def test_contrl_is_six_wall_clock_hours(self):
        assert contrl_due_at(MONDAY) == add_hours(MONDAY, 6)

    def test_add_hours_is_wall_clock_and_ignores_the_calendar(self):
        """GPKE's clock times survive a weekend; wall-clock hours run through it."""
        assert add_hours("2026-03-07T09:00:00Z", 24).startswith("2026-03-08")

    def test_next_werktag_at_takes_an_explicit_clock_time(self):
        assert next_werktag_at(MONDAY, "06:00") == "2026-03-03T06:00:00+01:00"


class TestCalendarEdges:
    def test_the_offset_follows_the_cet_cest_transition(self):
        """Rendering a deadline in UTC hides the hour that makes it correct."""
        assert deadline_at_werktage("2026-01-12T09:00:00Z", 1).endswith("+01:00")
        assert deadline_at_werktage("2026-07-13T09:00:00Z", 1).endswith("+02:00")

    def test_one_werktag_from_30_december_is_five_calendar_days(self):
        """The sharpest case in the calendar.

        31.12. and 01.01. are non-Werktage and 02./03.01. is a weekend, so any
        "n Werktage ≈ n + 2 calendar days" rule reports this one as breached
        three days early.
        """
        assert add_werktage("2026-12-30", 1) == "2027-01-04"
        assert (
            deadline_at_werktage("2026-12-30T09:00:00Z", 1) == "2027-01-04T17:00:00+01:00"
        )

    def test_a_bad_datetime_is_rejected_with_a_useful_message(self):
        with pytest.raises(ValueError, match="RFC 3339"):
            deadline_at_werktage("2026-03-02", 3)


class TestAssertions:
    def test_assert_deadline_is_takes_the_pid_and_finds_the_shape(self):
        assert_deadline_is(antwort_deadline(55001, MONDAY), received=MONDAY, pid=55001)
        assert_deadline_is(antwort_deadline(44001, MONDAY), received=MONDAY, pid=44001)

    def test_the_same_instant_in_another_offset_is_the_same_deadline(self):
        """A Frist carries the Berlin offset; a platform commonly reports UTC.

        `…T10:00:00Z` and `…T11:00:00+01:00` are one moment, and failing the
        first would report a correct deadline as a defect — while a string
        comparison would also *pass* a wrong instant that happens to render
        identically, which is the same bug in the loose direction.
        """
        assert_deadline_is(
            "2026-03-03T10:00:00Z", received=MONDAY, pid=55001
        )  # 11:00+01:00
        with pytest.raises(AssertionError):
            assert_deadline_is("2026-03-03T11:00:00Z", received=MONDAY, pid=55001)

    def test_asserting_a_gpke_deadline_with_werktage_arithmetic_fails(self):
        """The whole reason `pid=` exists."""
        with pytest.raises(AssertionError) as excinfo:
            assert_deadline_is(
                deadline_at_werktage(MONDAY, 1), received=MONDAY, pid=55001
            )
        assert "BK6-24-174" in str(excinfo.value), "the failure must cite the Frist"

    def test_the_werktage_escape_hatch_is_explicit(self):
        assert_deadline_is(deadline_at_werktage(MONDAY, 3), received=MONDAY, werktage=3)
        with pytest.raises(ValueError, match="exactly one of"):
            assert_deadline_is("x", received=MONDAY, pid=55001, werktage=3)
        with pytest.raises(ValueError, match="exactly one of"):
            assert_deadline_is("x", received=MONDAY)

    def test_asserting_a_deadline_for_an_unquantified_pid_says_so(self):
        with pytest.raises(ValueError, match="unknown, not unbounded"):
            assert_deadline_is("x", received=MONDAY, pid=44020)

    def test_assert_frist_met_measures_against_the_published_window(self):
        assert_frist_met(55001, received=MONDAY, answered_at="2026-03-03T10:59:00+01:00")
        with pytest.raises(AssertionError, match="was late"):
            assert_frist_met(
                55001, received=MONDAY, answered_at="2026-03-03T11:00:01+01:00"
            )


class TestInstantComparison:
    """A Frist is an instant, and RFC 3339 strings do not sort like one.

    `"…T10:30:00Z"` sorts before `"…T11:00:00+01:00"` while being half an hour
    later, so a string comparison passes a late answer whenever the deadline and
    the answer carry different offsets — which is the normal case, because a
    deadline is Europe/Berlin and a platform timestamps in UTC.
    """

    def test_a_late_answer_in_utc_is_still_late(self):
        # 11:00+01:00 is 10:00Z, so 10:30Z is thirty minutes past the Frist.
        with pytest.raises(AssertionError, match="was late"):
            assert_frist_met(55001, received=MONDAY, answered_at="2026-03-03T10:30:00Z")

    def test_an_answer_in_utc_inside_the_window_passes(self):
        assert_frist_met(55001, received=MONDAY, answered_at="2026-03-03T09:59:00Z")

    def test_a_naive_timestamp_is_refused(self):
        with pytest.raises(AssertionError, match="no UTC offset"):
            assert_frist_met(55001, received=MONDAY, answered_at="2026-03-03T09:00:00")
