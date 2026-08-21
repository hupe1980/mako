"""Simulated external counterparties.

Each simulator models the *observable behaviour* of a counterparty — what it
puts on the wire, and when it stays silent — rather than reimplementing its
internals. The interesting modes are the unhappy ones: an Ablehnung, a
Klärfall, an expired certificate, and above all silence, because deadline
handling is where regulated processes actually fail and it cannot be tested
against a partner that always answers.

Replies are **rendered EDIFACT interchanges**, built by the same Rust builders
the platform uses. That is what lets a test close the loop: feed the reply back
into the system under test and assert on what it does next. A simulator that
answered with a dictionary could only ever test the half of the exchange the
test itself wrote.

The simulators are plain objects with a `receive()` method, not servers. That
keeps them usable from a notebook or a load script, and lets an AS4 transport be
layered on top by whoever needs one instead of being a dependency of everyone
who does not.

A counterparty is modelled once it has a consumer. A simulator written ahead of
one encodes guesses about an interface nobody has implemented, and those guesses
are indistinguishable from requirements to the next reader.
"""

from .biko import MABIS_SUMMENZEITREIHE_PIDS, BikoSim, Klaerfall, Submission
from .imsys import (
    READING_QUALITIES,
    STEUERUNG_TAF,
    TAF_PROFILES,
    CertificateState,
    ClsChannel,
    ImsysSim,
    Zaehlerstandsgang,
)
from .marktpartner import (
    Answer,
    AntwortModus,
    Exchange,
    MarktpartnerSim,
    Reply,
    Rulebook,
    ablehnung_pid,
    answer_pids,
    bestaetigung_pid,
)

__all__ = [
    "MABIS_SUMMENZEITREIHE_PIDS",
    "READING_QUALITIES",
    "STEUERUNG_TAF",
    "TAF_PROFILES",
    "Answer",
    "AntwortModus",
    "BikoSim",
    "CertificateState",
    "ClsChannel",
    "Exchange",
    "ImsysSim",
    "Klaerfall",
    "MarktpartnerSim",
    "Reply",
    "Rulebook",
    "Submission",
    "Zaehlerstandsgang",
    "ablehnung_pid",
    "answer_pids",
    "bestaetigung_pid",
]
