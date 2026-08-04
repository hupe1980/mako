"""A Smart-Meter-Gateway (iMSys).

Models the compliance surface a platform must react to — TAF profile, CLS
channel state, certificate lifecycle, Zählerstandsgang delivery — not BSI
TR-03109 crypto. Reimplementing the crypto would be a second implementation of
something the SMGW already owns, and getting it subtly wrong would make tests
disagree with reality in the one place they must not.

What a platform actually has to handle is the *state machine*: a certificate
that expires mid-process, a CLS channel that will not open, a TAF that does not
deliver what was ordered. Those are here.
"""

from __future__ import annotations

import datetime as _dt
from dataclasses import dataclass, field
from typing import Literal

__all__ = ["CertificateState", "ClsChannel", "ImsysSim", "Zaehlerstandsgang"]

#: TAF profiles defined by BSI TR-03109-1 Anlage IV that carry meter data.
TAF_PROFILES = {
    "TAF-1": "Datensparsame Tarife (monatlicher Zählerstand)",
    "TAF-2": "Zeitvariable Tarife",
    "TAF-6": "Ablesung von Messwerten im Bedarfsfall",
    "TAF-7": "Zählerstandsgangmessung",
    "TAF-9": "Abruf der Ist-Einspeisung",
    "TAF-10": "Abruf von Netzzustandsdaten",
    "TAF-14": "Hochfrequente Messwertbereitstellung (§14a Steuerung)",
}

CertificateState = Literal["valid", "expiring", "expired", "revoked"]


@dataclass
class ClsChannel:
    """A CLS (Controllable Local Systems) channel — the §14a control path."""

    channel_id: str
    open: bool = False
    #: Why the last open attempt failed, when it did.
    last_error: str | None = None


@dataclass
class Zaehlerstandsgang:
    """One delivered meter-reading series."""

    melo_id: str
    taf: str
    #: ISO 8601 date the series covers.
    tag: str
    #: kWh per MTU, in order.
    werte: list[float] = field(default_factory=list)
    #: Periods the gateway could not supply — a real and testable condition.
    luecken: list[int] = field(default_factory=list)

    @property
    def vollstaendig(self) -> bool:
        """`True` when no period is missing."""
        return not self.luecken


class ImsysSim:
    """A Smart-Meter-Gateway whose compliance state the test controls.

    ::

        smgw = ImsysSim(melo_id="DE0006819497000000000000000001234", taf="TAF-7")
        smgw.expire_certificate_in(days=5)      # → "expiring", warn path
        smgw.deliver("2026-11-01", werte=[...], luecken=[41, 42])
        assert not smgw.letzte_lieferung.vollstaendig
    """

    #: Days before expiry at which a gateway starts warning. BSI TR-03109-1
    #: leaves the exact window to the operator; 30 days is common practice and
    #: is what `certificate_state` uses to distinguish "expiring" from "valid".
    WARN_WINDOW_DAYS = 30

    def __init__(
        self,
        *,
        melo_id: str,
        taf: str = "TAF-7",
        today: str = "2026-11-01",
        certificate_valid_until: str | None = None,
    ) -> None:
        if taf not in TAF_PROFILES:
            raise ValueError(
                f"unknown TAF profile {taf!r}; known: {sorted(TAF_PROFILES)}"
            )
        self.melo_id = melo_id
        self.taf = taf
        self._today = _dt.date.fromisoformat(today)
        self._valid_until = (
            _dt.date.fromisoformat(certificate_valid_until)
            if certificate_valid_until
            else self._today + _dt.timedelta(days=365)
        )
        self._revoked = False
        self.channels: dict[str, ClsChannel] = {}
        self.lieferungen: list[Zaehlerstandsgang] = []

    # ── Certificate lifecycle ─────────────────────────────────────────────────

    @property
    def certificate_state(self) -> CertificateState:
        """Current certificate state as a platform would observe it."""
        if self._revoked:
            return "revoked"
        if self._today > self._valid_until:
            return "expired"
        if (self._valid_until - self._today).days <= self.WARN_WINDOW_DAYS:
            return "expiring"
        return "valid"

    @property
    def certificate_valid_until(self) -> str:
        return self._valid_until.isoformat()

    def expire_certificate_in(self, *, days: int) -> ImsysSim:
        """Set the certificate to expire `days` from the simulated today.

        Negative `days` puts it in the past — the expired path.
        """
        self._valid_until = self._today + _dt.timedelta(days=days)
        return self

    def revoke_certificate(self) -> ImsysSim:
        """Revoke the certificate — terminal, and distinct from expiry."""
        self._revoked = True
        return self

    def advance(self, *, days: int) -> ImsysSim:
        """Move the gateway's clock, so expiry can be crossed mid-test."""
        self._today += _dt.timedelta(days=days)
        return self

    @property
    def today(self) -> str:
        return self._today.isoformat()

    # ── CLS channels (§14a EnWG steering) ─────────────────────────────────────

    def open_channel(self, channel_id: str) -> ClsChannel:
        """Open a CLS channel, or fail if the certificate does not permit it.

        A revoked or expired certificate must not yield a usable control path —
        this is the condition a §14a dispatch has to handle before it steers.
        """
        state = self.certificate_state
        channel = self.channels.setdefault(channel_id, ClsChannel(channel_id))
        if state in ("expired", "revoked"):
            channel.open = False
            channel.last_error = f"certificate {state}"
        else:
            channel.open = True
            channel.last_error = None
        return channel

    def close_channel(self, channel_id: str) -> ClsChannel:
        channel = self.channels.setdefault(channel_id, ClsChannel(channel_id))
        channel.open = False
        return channel

    # ── Zählerstandsgang delivery ─────────────────────────────────────────────

    def deliver(
        self,
        tag: str,
        *,
        werte: list[float],
        luecken: list[int] | None = None,
    ) -> Zaehlerstandsgang:
        """Record a delivered series for `tag` (ISO 8601).

        `luecken` are indices the gateway could not supply. A platform must
        substitute (Ersatzwertbildung) rather than treat a gap as zero, so the
        gap case needs to be producible.
        """
        if self.certificate_state in ("expired", "revoked"):
            raise RuntimeError(
                f"gateway certificate is {self.certificate_state}; a real SMGW "
                f"would not deliver — assert on certificate_state before calling"
            )
        gang = Zaehlerstandsgang(
            melo_id=self.melo_id,
            taf=self.taf,
            tag=tag,
            werte=list(werte),
            luecken=sorted(luecken or []),
        )
        self.lieferungen.append(gang)
        return gang

    @property
    def letzte_lieferung(self) -> Zaehlerstandsgang | None:
        return self.lieferungen[-1] if self.lieferungen else None

    def __repr__(self) -> str:
        return (
            f"ImsysSim(melo_id={self.melo_id!r}, taf={self.taf!r}, "
            f"certificate={self.certificate_state!r}, "
            f"lieferungen={len(self.lieferungen)})"
        )
