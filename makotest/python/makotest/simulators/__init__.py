"""Simulated external counterparties.

Each simulator models the *observable behaviour* of a counterparty — what it
answers, and when it stays silent — rather than reimplementing its internals.
The interesting modes are the unhappy ones: an Ablehnung, a Klärfall, an expired
certificate, and above all silence, because deadline handling is where regulated
processes actually fail and it cannot be tested against a partner that always
answers.

The simulators are plain objects with a `receive()` method, not servers. That
keeps them usable from a notebook or a load script, and lets an AS4 transport be
layered on top by whoever needs one instead of being a dependency of everyone who
does not.

`MaStRSim` and `UbaSim` are specified in the concept but deliberately not built:
neither integration exists in mako yet, and a simulator written before its
consumer would encode guesses about an interface nobody has implemented.
"""

from .biko import BikoSim, Klaerfall, Submission
from .imsys import CertificateState, ClsChannel, ImsysSim, Zaehlerstandsgang
from .marktpartner import (
    Answer,
    AntwortModus,
    Exchange,
    MarktpartnerSim,
    Rulebook,
    ablehnung_pid,
    answer_pids,
    bestaetigung_pid,
)

__all__ = [
    "Answer",
    "AntwortModus",
    "BikoSim",
    "CertificateState",
    "ClsChannel",
    "Exchange",
    "ImsysSim",
    "Klaerfall",
    "MarktpartnerSim",
    "Rulebook",
    "Submission",
    "Zaehlerstandsgang",
    "ablehnung_pid",
    "answer_pids",
    "bestaetigung_pid",
]
