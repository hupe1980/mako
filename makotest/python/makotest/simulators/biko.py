"""The Bilanzkoordinator.

Receives MaBiS Summenzeitreihen and answers with an APERAK: an acceptance, or a
Clearing-relevant rejection. The rejection path is the point — a Klärfall is
where a platform has to correlate the rejection back to the submitted period and
re-submit.

It refuses anything not addressed to a Bilanzkoordinator. A simulator that
accepted a UTILMD as a Summenzeitreihe would agree to something no real
counterparty does, and every assertion downstream of that acceptance would be
meaningless.

An interchange carries several Summenzeitreihen, each its own settlement, and
each is assessed and acknowledged separately: `RFF+ACW` names one UNH, so one
APERAK could only ever speak for one of them.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

from .._native import build_aperak_for, build_interchange, validate_edifact
from .marktpartner import Reply, _wire_text

__all__ = ["MABIS_SUMMENZEITREIHE_PIDS", "BikoSim", "Klaerfall", "Submission"]

#: The MSCONS Prüfidentifikatoren a Bilanzkoordinator receives under MaBiS.
#:
#: 13003 is the Summenzeitreihe („Summenzeitreihen und Ausfallarbeitssummen",
#: MSCONS AHB §5); 13010–13012 are the normiertes Profil, the Profilschar and
#: the TEP, which are BK-Treue/MaBiS settlement profile data. The rest of the
#: 13xxx band is **Messwesen** — 13002 is a Gas Zählerstand and 13008 a Gas
#: Lastgang, both addressed to a Netzbetreiber. A BIKO that accepted any MSCONS
#: would accept a meter reading as a balance-group settlement, and every
#: assertion downstream of that acceptance would be meaningless.
#:
#: MaBiS is Strom-only, so there is no Gas counterpart to this set.
MABIS_SUMMENZEITREIHE_PIDS = frozenset({13003, 13010, 13011, 13012})

KlaerfallGrund = Literal[
    "zeitreihe_unvollstaendig",
    "bilanzkreis_unbekannt",
    "periode_bereits_abgerechnet",
    "summe_weicht_ab",
]


@dataclass(frozen=True)
class Klaerfall:
    """A Clearing-relevant rejection."""

    grund: KlaerfallGrund
    #: Free-text detail the platform surfaces to an operator.
    detail: str = ""
    #: APERAK error code, when the rejection carries one.
    erc: str = "Z10"


@dataclass
class Submission:
    """One Abrechnungssummenzeitreihe the simulator received."""

    pid: int | None
    bilanzkreis: str | None
    valid: bool
    accepted: bool
    klaerfall: Klaerfall | None = None
    #: `False` when the PID had no AHB rules, so `valid` was decided vacuously.
    checked: bool = True
    #: Why a submission was refused outright, when it was not even a Klärfall.
    refused: str | None = None


class BikoSim:
    """A Bilanzkoordinator that accepts or raises a Klärfall.

    ::

        biko = BikoSim(mp_id="9979999000002", reference_date="2025-10-01")
        biko.reject_next(Klaerfall("summe_weicht_ab", detail="Δ 12,4 kWh"))
        first  = biko.receive(mscons_wire)   # reply.ack carries the Klärfall
        second = biko.receive(mscons_wire)   # accepted — the queue drained
    """

    def __init__(
        self,
        *,
        mp_id: str,
        reference_date: str,
        accept_by_default: bool = True,
    ) -> None:
        self.mp_id = mp_id
        self.reference_date = reference_date
        self.accept_by_default = accept_by_default
        self._sent = 0
        self.submissions: list[Submission] = []
        self._pending: list[Klaerfall] = []
        self._by_bilanzkreis: dict[str, Klaerfall] = {}

    # ── Configuration ─────────────────────────────────────────────────────────

    def reject_next(self, klaerfall: Klaerfall) -> BikoSim:
        """Raise `klaerfall` on the next submission, then return to default.

        Queued rather than sticky, so a test can assert that the re-submission
        after Clearing succeeds — which is the whole point of the Klärfall path.
        """
        self._pending.append(klaerfall)
        return self

    def reject_bilanzkreis(self, bilanzkreis: str, klaerfall: Klaerfall) -> BikoSim:
        """Always raise `klaerfall` for submissions naming `bilanzkreis`."""
        self._by_bilanzkreis[bilanzkreis] = klaerfall
        return self

    # ── Behaviour ─────────────────────────────────────────────────────────────

    def receive(self, raw: bytes, *, bilanzkreis: str | None = None) -> Reply:
        """Handle one inbound MSCONS submission and answer with an APERAK.

        Returns the same `Reply` a Marktpartner does, so a test does not have to
        remember which simulator hands back bytes and which hands back an
        envelope. A BIKO sends no business answer, so only the acknowledgement
        half is populated.

        `bilanzkreis` is taken from the caller rather than parsed out of the
        interchange: which segment carries it differs by MSCONS profile, and a
        simulator that guessed wrong would fail tests for the wrong reason.
        """
        if not raw.lstrip().startswith(b"UNB"):
            raise ValueError(
                "a Bilanzkoordinator receives interchanges, not bare messages — "
                "wrap the message with build_interchange() first"
            )
        report = validate_edifact(raw, self.reference_date)
        if not report.messages:
            raise ValueError(
                "the interchange carries no message — there is nothing to "
                "acknowledge and nothing to settle"
            )
        sender = report.envelope.sender_id if report.envelope else self.mp_id

        # One APERAK per message: `RFF+ACW` names a single UNH, so a single
        # acknowledgement could only ever speak for one Summenzeitreihe — and an
        # interchange routinely carries several, each its own settlement.
        acknowledgements = [
            self._assess(message, report, bilanzkreis) for message in report.messages
        ]
        return self._aperak(raw, sender, acknowledgements)

    def _assess(
        self, message, report, bilanzkreis: str | None
    ) -> tuple[str | None, str | None]:
        """Settle one submitted message and return its `(ERC, text)`."""
        pid = message.pruefidentifikator
        checked = message.rules_applied

        refusal = self._misroute_reason(message)
        if refusal is not None:
            self._log(
                Submission(
                    pid,
                    bilanzkreis,
                    message.is_valid,
                    False,
                    checked=checked,
                    refused=refusal,
                )
            )
            return "Z18", _wire_text(refusal)

        if not message.is_valid or not report.is_valid:
            first = next(iter(message.errors), None) or next(iter(report.errors), None)
            self._log(Submission(pid, bilanzkreis, False, False, checked=checked))
            return "Z10", (_wire_text(first.message) if first else "invalid")

        klaerfall = self._klaerfall_for(bilanzkreis)
        accepted = klaerfall is None
        self._log(
            Submission(pid, bilanzkreis, True, accepted, klaerfall, checked=checked)
        )
        if accepted:
            return None, None
        return klaerfall.erc, _wire_text(f"{klaerfall.grund}: {klaerfall.detail}")

    def _misroute_reason(self, message) -> str | None:
        if message.message_type != "MSCONS":
            return f"{message.message_type} is not a MaBiS Summenzeitreihe"
        if message.pruefidentifikator not in MABIS_SUMMENZEITREIHE_PIDS:
            return f"PID {message.pruefidentifikator} is not addressed to a BIKO"
        return None

    def _klaerfall_for(self, bilanzkreis: str | None) -> Klaerfall | None:
        if bilanzkreis is not None and bilanzkreis in self._by_bilanzkreis:
            return self._by_bilanzkreis[bilanzkreis]
        if self._pending:
            return self._pending.pop(0)
        if not self.accept_by_default:
            return Klaerfall("zeitreihe_unvollstaendig", detail="default reject")
        return None

    def _aperak(
        self, raw: bytes, to: str, outcomes: list[tuple[str | None, str | None]]
    ) -> Reply:
        # UNB DE0020 identifies the interchange to the receiver. A partner that
        # reused one would be sending a duplicate every conformant receiver is
        # entitled to discard — so it is a counter, not a constant, and a counter
        # rather than a clock so a scenario stays byte-reproducible.
        self._sent += 1
        wire = build_interchange(
            sender=self.mp_id,
            receiver=to,
            dar=f"BK{self._sent:06d}",
            on=self.reference_date,
            messages=[
                build_aperak_for(
                    raw,
                    on=self.reference_date,
                    error_code=erc,
                    error_text=text,
                    message_ref=str(index + 1),
                    message_index=index,
                )
                for index, (erc, text) in enumerate(outcomes)
            ],
        )
        return Reply(
            ack=wire,
            ack_kind="APERAK",
            # The interchange is accepted only when every series in it was.
            ack_positive=all(erc is None for erc, _ in outcomes),
            ercs=tuple(erc for erc, _ in outcomes),
        )

    def _log(self, submission: Submission) -> None:
        self.submissions.append(submission)

    # ── Inspection ────────────────────────────────────────────────────────────

    @property
    def klaerfaelle(self) -> list[Submission]:
        """Submissions that were rejected into Clearing."""
        return [s for s in self.submissions if s.klaerfall is not None]

    @property
    def accepted(self) -> list[Submission]:
        return [s for s in self.submissions if s.accepted]

    @property
    def unvalidated_submissions(self) -> list[int]:
        """PIDs received whose AHB rules were never applied."""
        return [s.pid for s in self.submissions if s.pid is not None and not s.checked]

    def __repr__(self) -> str:
        return (
            f"BikoSim(mp_id={self.mp_id!r}, submissions={len(self.submissions)}, "
            f"klaerfaelle={len(self.klaerfaelle)})"
        )
