"""The opposing Marktpartner.

Receives an interchange and answers the way a real counterparty would: a CONTRL
acknowledgement, then the AHB answer for that Prüfidentifikator — a Bestätigung,
an Ablehnung, or nothing at all.

The third mode is the one worth having. A platform that never sees silence is
never tested against its own Fristen, and deadline handling is where regulated
processes actually fail. `.timeout()` is how you exercise it.

The simulator is a plain object with a `receive()` method, not a server. That
keeps it usable from a notebook and from a load script, and lets the AS4
transport be layered on top by whoever needs it rather than being a dependency
of everyone who does not.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Literal

from .._native import (
    ablehnung_pid,
    answer_pids,
    bestaetigung_pid,
    message_types_of,
    pid_has_ahb_rules,
    validate_edifact,
)

if TYPE_CHECKING:
    from .._native import ValidationReport

__all__ = [
    "Answer",
    "AntwortModus",
    "Exchange",
    "MarktpartnerSim",
    "Rulebook",
    "ablehnung_pid",
    "answer_pids",
    "bestaetigung_pid",
]

AntwortModus = Literal["bestaetigung", "ablehnung", "timeout"]

@dataclass(frozen=True)
class Answer:
    """What the simulator will send back for one Prüfidentifikator."""

    modus: AntwortModus
    #: Answer PID. `None` for `timeout`, and for a mode with no AHB answer PID.
    pid: int | None = None
    #: EBD rejection code, e.g. `"A06"` (conflicting supply). Ablehnung only.
    erc: str | None = None
    #: Extra fields echoed into the answer payload, e.g. `zuordnungsbeginn`.
    felder: dict[str, str] = field(default_factory=dict)
    #: Emit a CONTRL before the business answer, as a real partner does.
    contrl: bool = True


class Rulebook:
    """Fluent binding of one request PID to its answer.

    Returned by :meth:`MarktpartnerSim.on`; each terminal call registers the
    answer and returns the simulator so calls can be chained.
    """

    def __init__(self, sim: MarktpartnerSim, pid: int) -> None:
        self._sim = sim
        self._pid = pid

    def _resolve(self, *, accepted: bool) -> int | None:
        """The AHB answer PID, from the shared table the platform also uses."""
        return bestaetigung_pid(self._pid) if accepted else ablehnung_pid(self._pid)

    def bestaetigung(self, **felder: str) -> MarktpartnerSim:
        """Answer with the AHB Bestätigung for this PID."""
        pid = self._resolve(accepted=True)
        if pid is None:
            raise ValueError(
                f"PID {self._pid} has no Bestätigung in the AHB answer table. "
                f"Either it is not a request PID, or the triple is missing — "
                f"pass an explicit answer with .antwort(pid=...)."
            )
        self._sim._answers[self._pid] = Answer("bestaetigung", pid=pid, felder=felder)
        return self._sim

    def ablehnung(self, *, erc: str | None = None, **felder: str) -> MarktpartnerSim:
        """Answer with the AHB Ablehnung, optionally carrying an EBD code."""
        pid = self._resolve(accepted=False)
        if pid is None:
            raise ValueError(
                f"PID {self._pid} has no Ablehnung in the AHB answer table. "
                f"Pass an explicit answer with .antwort(pid=...)."
            )
        self._sim._answers[self._pid] = Answer(
            "ablehnung", pid=pid, erc=erc, felder=felder
        )
        return self._sim

    def timeout(self) -> MarktpartnerSim:
        """Send nothing — not even a CONTRL.

        This is how a Frist is tested: the platform must escalate on its own
        deadline rather than waiting for a peer that never answers.
        """
        self._sim._answers[self._pid] = Answer("timeout", contrl=False)
        return self._sim

    def antwort(
        self,
        *,
        pid: int,
        modus: AntwortModus = "bestaetigung",
        erc: str | None = None,
        **felder: str,
    ) -> MarktpartnerSim:
        """Answer with an explicit PID, bypassing the answer table.

        For adversarial cases — answering with the wrong PID is a thing real
        counterparties do, and a platform should reject it.
        """
        self._sim._answers[self._pid] = Answer(modus, pid=pid, erc=erc, felder=felder)
        return self._sim


@dataclass
class Exchange:
    """One request/answer pair the simulator handled."""

    request_pid: int | None
    request_valid: bool
    answer: Answer | None
    #: `False` when the request carried a PID the profiles have no rules for,
    #: so `request_valid` was decided vacuously.
    request_checked: bool = True


class MarktpartnerSim:
    """A counterparty that answers per the AHB.

    ::

        nb = MarktpartnerSim(mp_id="9900357000004", rolle="NB")
        nb.on(55001).bestaetigung(zuordnungsbeginn="2026-11-01")
        nb.on(55004).ablehnung(erc="A06")
        nb.on(55016).timeout()

        answer = nb.receive(interchange_bytes)
    """

    def __init__(
        self,
        *,
        mp_id: str,
        rolle: str,
        reference_date: str | None = None,
        strict: bool = True,
    ) -> None:
        """
        `strict` rejects a request that fails AHB validation instead of
        answering it — which is what a conformant partner does. Set it `False`
        to test how the platform handles a partner that answers anyway.
        """
        self.mp_id = mp_id
        self.rolle = rolle
        self.reference_date = reference_date
        self.strict = strict
        self._answers: dict[int, Answer] = {}
        self.exchanges: list[Exchange] = []

    # ── Configuration ─────────────────────────────────────────────────────────

    def on(self, pid: int) -> Rulebook:
        """Bind an answer to the request Prüfidentifikator `pid`."""
        return Rulebook(self, pid)

    # ── Behaviour ─────────────────────────────────────────────────────────────

    def receive(self, raw: bytes) -> dict | None:
        """Handle one inbound interchange.

        Returns the answer envelope, or `None` when the configured mode is
        `timeout` (or no answer is bound for the request's PID — an unconfigured
        partner is a silent one, which is the safe default for Frist testing).
        """
        report: ValidationReport = validate_edifact(raw, self.reference_date)
        pid = report.pruefidentifikator
        # The parsed report names the type authoritatively; the profile lookup
        # is only a fallback, and it can return several (29xxx is declared by
        # both APERAK and COMDIS), so take the first.
        mt = report.message_type or (
            next(iter(message_types_of(pid)), None) if pid else None
        )

        checked = bool(pid) and bool(mt) and pid_has_ahb_rules(mt, pid)

        answer = self._answers.get(pid) if pid is not None else None
        self.exchanges.append(
            Exchange(
                request_pid=pid,
                request_valid=report.is_valid,
                answer=answer,
                request_checked=checked,
            )
        )

        if self.strict and not report.is_valid:
            return {
                "kind": "CONTRL",
                "ack": False,
                "from": self.mp_id,
                "request_pid": pid,
                "findings": [f.message for f in report.findings],
            }

        if answer is None or answer.modus == "timeout":
            return None

        return {
            "kind": "UTILMD" if mt is None else mt,
            "contrl": answer.contrl,
            "from": self.mp_id,
            "rolle": self.rolle,
            "request_pid": pid,
            "pid": answer.pid,
            "modus": answer.modus,
            "erc": answer.erc,
            **answer.felder,
        }

    # ── Inspection ────────────────────────────────────────────────────────────

    @property
    def unvalidated_requests(self) -> list[int]:
        """PIDs received whose AHB rules were never applied.

        Non-empty means some assertion in the test passed vacuously: the
        request "validated" because the profile set has no rules for that PID,
        not because it was correct.
        """
        return [
            e.request_pid
            for e in self.exchanges
            if e.request_pid is not None and not e.request_checked
        ]

    def __repr__(self) -> str:
        return (
            f"MarktpartnerSim(mp_id={self.mp_id!r}, rolle={self.rolle!r}, "
            f"answers={len(self._answers)}, exchanges={len(self.exchanges)})"
        )
