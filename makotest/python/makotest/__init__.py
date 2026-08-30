"""makotest — test & simulation toolkit for German market-communication platforms.

Generates regulator-conformant inputs (EDIFACT, EPEX price curves, meter data),
simulates the external counterparties a MaKo platform talks to, and asserts on
the result.

Not mako-specific: everything it drives is a public wire contract (EDIFACT over
AS4, REST, CloudEvents), so it can exercise any MaKo implementation.

The wire-format primitives come from the same Rust crates the platform runs, so
`makotest` and the system under test can never disagree about what "valid"
means, nor about when a Frist expires::

    >>> from makotest import malo_from_base, add_werktage, antwort_obligation
    >>> malo_from_base("5123869601")
    '51238696012'
    >>> add_werktage("2026-12-24", 2)   # skips Christmas and the weekend
    '2026-12-29'
    >>> antwort_obligation(55001).clock_time   # GPKE: a clock time, not n × 24 h
    '11:00'

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
    AntwortCode,
    AntwortObligation,
    Envelope,
    Finding,
    MessageReport,
    Positionsfehler,
    UtilmdTransaction,
    ValidationReport,
    Vorgang,
    ablehnung_pid,
    add_hours,
    add_werktage,
    answer_pids,
    antwort_code,
    antwort_codes,
    antwort_codes_for_pid,
    antwort_deadline,
    antwort_obligation,
    antwort_obligations,
    aperak_gas_folgeprozess_due_at,
    aperak_gas_initialprozess_due_at,
    aperak_strom_due_at,
    berlin_day_bounds,
    berlin_instant,
    berlin_mtu_count,
    bestaetigung_pid,
    bilanzierungsgebiet_from_prefix,
    bilanzierungsgebiet_is_valid,
    bilanzkreis_from_prefix,
    bilanzkreis_is_valid,
    bo4e_schema_version,
    build_answer,
    build_aperak,
    build_aperak_for,
    build_contrl,
    build_contrl_for,
    build_iftsta,
    build_interchange,
    build_mscons,
    build_orders,
    build_ordrsp,
    build_quotes,
    build_remadv,
    build_utilmd,
    cloudevent_core_attributes,
    cloudevent_json_members,
    contrl_due_at,
    deadline_at_werktage,
    eic_from_prefix,
    eic_is_valid,
    eic_type_char,
    end_of_werktag_after,
    entscheidungsbaeume,
    event_matches,
    event_type_exists,
    event_types,
    event_types_matching,
    format_303,
    format_versions,
    is_valid_extension_key,
    is_werktag,
    malo_check_digit,
    malo_from_base,
    malo_is_valid,
    melo_is_valid,
    message_types_of,
    mp_id_authority,
    mp_id_check_digit_schemes,
    mp_id_from_base,
    mp_id_is_valid,
    mp_id_unb_qualifier,
    next_werktag,
    next_werktag_at,
    parse_cloudevent_time,
    pid_carrying_message_types,
    pid_has_ahb_rules,
    pruefidentifikatoren,
    release_for,
    releases,
    resource_id_from_base,
    resource_id_is_valid,
    resource_id_kinds,
    validate_edifact,
)
from .assertions import (
    assert_answer_pid,
    assert_antwort_code,
    assert_bo4e_generation_matches,
    assert_cloudevent,
    assert_deadline_is,
    assert_edifact_valid,
    assert_event_emitted,
    assert_frist_met,
    assert_invoice_reconciles,
    assert_no_event_emitted,
    assert_rule_fires,
    assert_rules_applied,
    find_events,
)
from .simulators import BikoSim, ImsysSim, Klaerfall, MarktpartnerSim, Reply

__all__ = [
    "AntwortCode",
    "AntwortObligation",
    "BikoSim",
    "Envelope",
    "Finding",
    "ImsysSim",
    "Klaerfall",
    "MarktpartnerSim",
    "MessageReport",
    "Positionsfehler",
    "Reply",
    "UtilmdTransaction",
    "ValidationReport",
    "Vorgang",
    "ablehnung_pid",
    "add_hours",
    "add_werktage",
    "answer_pids",
    "antwort_code",
    "antwort_codes",
    "antwort_codes_for_pid",
    "antwort_deadline",
    "antwort_obligation",
    "antwort_obligations",
    "aperak_gas_folgeprozess_due_at",
    "aperak_gas_initialprozess_due_at",
    "aperak_strom_due_at",
    "assert_answer_pid",
    "assert_antwort_code",
    "assert_bo4e_generation_matches",
    "assert_cloudevent",
    "assert_deadline_is",
    "assert_edifact_valid",
    "assert_event_emitted",
    "assert_frist_met",
    "assert_invoice_reconciles",
    "assert_no_event_emitted",
    "assert_rule_fires",
    "assert_rules_applied",
    "berlin_day_bounds",
    "berlin_instant",
    "berlin_mtu_count",
    "bestaetigung_pid",
    "bilanzierungsgebiet_from_prefix",
    "bilanzierungsgebiet_is_valid",
    "bilanzkreis_from_prefix",
    "bilanzkreis_is_valid",
    "bo4e_schema_version",
    "build_answer",
    "build_aperak",
    "build_aperak_for",
    "build_contrl",
    "build_contrl_for",
    "build_iftsta",
    "build_interchange",
    "build_mscons",
    "build_orders",
    "build_ordrsp",
    "build_quotes",
    "build_remadv",
    "build_utilmd",
    "cloudevent_core_attributes",
    "cloudevent_json_members",
    "contrl_due_at",
    "deadline_at_werktage",
    "eic_from_prefix",
    "eic_is_valid",
    "eic_type_char",
    "end_of_werktag_after",
    "entscheidungsbaeume",
    "event_matches",
    "event_type_exists",
    "event_types",
    "event_types_matching",
    "find_events",
    "format_303",
    "format_versions",
    "is_valid_extension_key",
    "is_werktag",
    "malo_check_digit",
    "malo_from_base",
    "malo_is_valid",
    "melo_is_valid",
    "message_types_of",
    "mp_id_authority",
    "mp_id_check_digit_schemes",
    "mp_id_from_base",
    "mp_id_is_valid",
    "mp_id_unb_qualifier",
    "next_werktag",
    "next_werktag_at",
    "parse_cloudevent_time",
    "pid_carrying_message_types",
    "pid_has_ahb_rules",
    "pruefidentifikatoren",
    "release_for",
    "releases",
    "resource_id_from_base",
    "resource_id_is_valid",
    "resource_id_kinds",
    "validate_edifact",
]

try:  # pragma: no cover - packaging metadata lookup
    from importlib.metadata import version as _pkg_version

    #: Tracks `workspace.package.version` via Cargo.toml — never hardcoded here,
    #: so the wheel and the crates it binds cannot report different versions.
    __version__ = _pkg_version("makotest")
except Exception:  # not installed (e.g. running from a source checkout)
    __version__ = "0.0.0+unknown"
