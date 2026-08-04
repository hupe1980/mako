"""The Bilanzkoordinator.

Receives MSCONS Abrechnungssummenzeitreihen (PID 13003) and answers with a
CONTRL, then either an acceptance or a Clearing-relevant rejection.

The rejection path is the point. A Bilanzkreis submission that is merely
*accepted* tells you the happy path works; a Klärfall is where a platform has to
correlate the rejection back to the submitted period and re-submit, and that is
the behaviour worth testing.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

from .._native import pid_has_ahb_rules, validate_edifact

__all__ = ["BikoSim", "Klaerfall", "Submission"]

#: BDEW MSCONS PID for the Abrechnungssummenzeitreihe (MaBiS).
ABRECHNUNGSSUMMENZEITREIHE_PID = 13003

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
    erc: str | None = None


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


class BikoSim:
    """A Bilanzkoordinator that accepts or raises a Klärfall.

    ::

        biko = BikoSim(mp_id="9979999000007")
        biko.reject_next(Klaerfall("summe_weicht_ab", detail="Δ 12,4 kWh"))
        answer = biko.receive(mscons_bytes)      # → APERAK with the Klärfall
        answer = biko.receive(mscons_bytes)      # → accepted (queue drained)
    """

    def __init__(
        self,
        *,
        mp_id: str,
        reference_date: str | None = None,
        accept_by_default: bool = True,
    ) -> None:
        self.mp_id = mp_id
        self.reference_date = reference_date
        self.accept_by_default = accept_by_default
        self.submissions: list[Submission] = []
        self._pending: list[Klaerfall] = []
        self._by_bilanzkreis: dict[str, Klaerfall] = {}

    # ── Configuration ─────────────────────────────────────────────────────────

    def reject_next(self, klaerfall: Klaerfall) -> BikoSim:
        """Raise `klaerfall` on the next submission, then return to default.

        Queued rather than sticky so a test can assert the re-submission after
        a Klärfall succeeds — the whole point of the Klärfall path.
        """
        self._pending.append(klaerfall)
        return self

    def reject_bilanzkreis(self, bilanzkreis: str, klaerfall: Klaerfall) -> BikoSim:
        """Always raise `klaerfall` for submissions naming `bilanzkreis`."""
        self._by_bilanzkreis[bilanzkreis] = klaerfall
        return self

    # ── Behaviour ─────────────────────────────────────────────────────────────

    def receive(self, raw: bytes, *, bilanzkreis: str | None = None) -> dict:
        """Handle one inbound MSCONS submission.

        `bilanzkreis` is taken from the caller rather than parsed out of the
        interchange: which segment carries it differs by MSCONS profile, and a
        simulator that guessed wrong would fail tests for the wrong reason.
        """
        report = validate_edifact(raw, self.reference_date)
        pid = report.pruefidentifikator
        checked = bool(pid) and pid_has_ahb_rules("MSCONS", pid)

        if not report.is_valid:
            sub = Submission(pid, bilanzkreis, False, False, checked=checked)
            self.submissions.append(sub)
            return {
                "kind": "CONTRL",
                "ack": False,
                "from": self.mp_id,
                "findings": [f.message for f in report.findings],
            }

        klaerfall: Klaerfall | None = None
        if bilanzkreis is not None and bilanzkreis in self._by_bilanzkreis:
            klaerfall = self._by_bilanzkreis[bilanzkreis]
        elif self._pending:
            klaerfall = self._pending.pop(0)
        elif not self.accept_by_default:
            klaerfall = Klaerfall("zeitreihe_unvollstaendig", detail="default reject")

        accepted = klaerfall is None
        self.submissions.append(
            Submission(pid, bilanzkreis, True, accepted, klaerfall, checked=checked)
        )

        if accepted:
            return {
                "kind": "APERAK",
                "ack": True,
                "from": self.mp_id,
                "pid": pid,
                "bilanzkreis": bilanzkreis,
            }
        return {
            "kind": "APERAK",
            "ack": False,
            "from": self.mp_id,
            "pid": pid,
            "bilanzkreis": bilanzkreis,
            "klaerfall": klaerfall.grund,
            "detail": klaerfall.detail,
            "erc": klaerfall.erc,
        }

    # ── Inspection ────────────────────────────────────────────────────────────

    @property
    def klaerfaelle(self) -> list[Submission]:
        """Submissions that were rejected into Clearing."""
        return [s for s in self.submissions if s.klaerfall is not None]

    @property
    def unvalidated_submissions(self) -> list[int]:
        """PIDs received whose AHB rules were never applied."""
        return [s.pid for s in self.submissions if s.pid is not None and not s.checked]

    def __repr__(self) -> str:
        return (
            f"BikoSim(mp_id={self.mp_id!r}, submissions={len(self.submissions)}, "
            f"klaerfaelle={len(self.klaerfaelle)})"
        )
