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
from decimal import Decimal
from typing import Literal

from .._native import (
    berlin_day_bounds,
    berlin_mtu_count,
    build_mscons,
    format_303,
)

__all__ = [
    "READING_QUALITIES",
    "STEUERUNG_TAF",
    "TAF_PROFILES",
    "CertificateState",
    "ClsChannel",
    "ImsysSim",
    "Zaehlerstandsgang",
]

#: Tarifanwendungsfälle a Smart-Meter-Gateway can be operated under.
#:
#: TAF 1–14 are defined by BSI TR-03109-1; TAF 16 by the separate
#: Implementierungshinweis „Ereignisbasierte Tarifanwendungsfälle TAF5 und
#: TAF16". There is no TAF 15.
#:
#: The names are the official German ones rather than a paraphrase, because the
#: number alone is ambiguous in exactly the places it matters. **TAF 11 is the
#: §14a EnWG steering case** and the one behind a CLS channel. TAF 14 is a
#: high-resolution read-out for value-added services and cannot steer at all;
#: ordering it and expecting to steer is a configuration error a platform has to
#: be able to detect. TAF 8 is about **Leistungs**extremwerte over a billing
#: period, not about generation metering — that is TAF 9.
TAF_PROFILES = {
    "TAF-1": "Datensparsame Tarife",
    "TAF-2": "Zeitvariable Tarife",
    "TAF-3": "Lastvariable Tarife",
    "TAF-4": "Verbrauchsvariable Tarife",
    "TAF-5": "Ereignisvariable Tarife",
    "TAF-6": "Ablesung von Messwerten im Bedarfsfall",
    "TAF-7": "Zählerstandsgangmessung",
    "TAF-8": "Erfassung von Extremwerten",
    "TAF-9": "Abruf der Ist-Einspeisung",
    "TAF-10": "Abruf von Netzzustandsdaten",
    "TAF-11": "Steuerung von unterbrechbaren Verbrauchseinrichtungen und "
    "Erzeugungsanlagen",
    "TAF-12": "Prepaid-Tarif",
    "TAF-13": "Letztverbraucher-Visualisierung",
    "TAF-14": "Hochfrequente Messwertbereitstellung für Mehrwertdienste",
    "TAF-16": "Ereignisbasierte Zählerstandserfassung",
}

#: Reading qualities a gateway can stamp on an interval, as the BDEW
#: Messwertstatus vocabulary a platform's ingest accepts them.
#:
#: The distinction that decides money: a `MEASURED` value is billable as it
#: stands, a `SUBSTITUTED` one is an Ersatzwert the Messstellenbetreiber formed
#: because measurement failed (§ 60 Abs. 2 MsbG), and a `FAULTY` one must not be
#: billed at all and obliges the platform to form one. A gateway that could only
#: deliver unqualified numbers could not exercise any of that.
READING_QUALITIES = (
    "MEASURED",
    "ESTIMATED",
    "SUBSTITUTED",
    "CALCULATED",
    "CORRECTED",
    "PRELIMINARY",
    "FAULTY",
    "UNKNOWN",
)

#: The Tarifanwendungsfall that carries a control path — §14a EnWG steering.
STEUERUNG_TAF = "TAF-11"

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
    """One delivered meter-reading series, laid out over a Europe/Berlin day.

    The series has exactly as many values as the day has market time units — 96
    on an ordinary day, **92** on the short March day and **100** on the long
    October one. A series that ignored that would run four intervals past
    midnight in March and stop an hour early in October, and both errors land
    mid-day where the curve still looks plausible.
    """

    melo_id: str
    taf: str
    #: ISO 8601 date the series covers.
    tag: str
    #: kWh per MTU, in order.
    werte: list[float] = field(default_factory=list)
    #: Periods the gateway could not supply — a real and testable condition.
    luecken: list[int] = field(default_factory=list)
    #: `{index: quality}` for periods the gateway delivered under something other
    #: than `MEASURED` — see :data:`READING_QUALITIES`.
    qualitaeten: dict[int, str] = field(default_factory=dict)
    #: Minutes per interval. 15 for a TAF-7 Zählerstandsgang.
    mtu_minutes: int = 15

    def __post_init__(self) -> None:
        expected = berlin_mtu_count(self.tag, self.mtu_minutes)
        if len(self.werte) != expected:
            raise ValueError(
                f"{self.tag} is a {expected * self.mtu_minutes // 60}-hour "
                f"Europe/Berlin day, so a {self.mtu_minutes}-minute series has "
                f"{expected} values, not {len(self.werte)}. Ask "
                f"berlin_mtu_count({self.tag!r}, {self.mtu_minutes}) rather than "
                f"assuming 96."
            )
        out_of_range = [i for i in self.luecken if not 0 <= i < expected]
        if out_of_range:
            raise ValueError(
                f"Lücken {out_of_range} lie outside the {expected} periods of "
                f"{self.tag}. An index nothing lands on removes no interval, so "
                f"the gap the test meant to create would silently not exist."
            )
        unknown = sorted(set(self.qualitaeten.values()) - set(READING_QUALITIES))
        if unknown:
            raise ValueError(
                f"unknown reading quality {unknown}; known: {list(READING_QUALITIES)}"
            )
        stamped = [i for i in self.qualitaeten if not 0 <= i < expected]
        if stamped:
            raise ValueError(f"quality indices {stamped} lie outside {self.tag}")

    @property
    def vollstaendig(self) -> bool:
        """`True` when no period is missing."""
        return not self.luecken

    @property
    def mtu_count(self) -> int:
        """Market time units this delivery day has."""
        return berlin_mtu_count(self.tag, self.mtu_minutes)

    def as_mscons(
        self,
        *,
        pruefidentifikator: int,
        sender_mp_id: str,
        receiver_mp_id: str,
        on: str,
        malo_id: str | None = None,
        obis_code: str = "1-0:1.29.0",
        qualifier: str = "220",
        unit: str = "KWH",
        message_ref: str = "1",
    ) -> bytes:
        """The delivery as an MSCONS **message** (`UNH`…`UNT`).

        One `QTY` per delivered interval, each carrying its own measurement
        period. That is the distinction that matters: a bare `QTY` has no time
        reference, so the receiver cannot place the value on the settlement
        grid — and the AHB does **not** reject it, so a Lastgang assembled from
        flat quantities validates while being unusable. Building it here is what
        keeps a test from producing that message by accident.

        Gaps are **absent** intervals, exactly as in `as_direct_push`: the
        receiver forms an Ersatzwert, and a zero would be settled against.

        `malo_id` names the Marktlokation when the series is reported against
        one (13025 is „Lastgang Marktlokation, Tranche"); it defaults to this
        gateway's Messlokation, which is what 13018 reports against.

        Wrap the result with `build_interchange()` — a message is not sendable
        on its own.
        """
        gaps = set(self.luecken)
        start, _ = berlin_day_bounds(self.tag)
        day = _dt.datetime.fromisoformat(start)
        step = _dt.timedelta(minutes=self.mtu_minutes)
        intervals = [
            (
                qualifier,
                f"{round(Decimal(str(value)), 4):f}",
                unit,
                format_303((day + i * step).isoformat()),
                format_303((day + (i + 1) * step).isoformat()),
            )
            for i, value in enumerate(self.werte)
            if i not in gaps
        ]
        if not intervals:
            raise ValueError(
                f"every interval of {self.tag} is a Lücke, so there is no value "
                f"to report — an MSCONS with no QTY is not a delivery"
            )
        return build_mscons(
            pruefidentifikator,
            sender_mp_id,
            receiver_mp_id,
            malo_id or self.melo_id,
            intervals=intervals,
            on=on,
            obis=obis_code,
            message_ref=message_ref,
        )

    def as_direct_push(
        self,
        *,
        sender_mp_id: str,
        obis_code: str = "1-0:1.29.0",
        session_id: str | None = None,
    ) -> dict[str, object]:
        """The request **body** a platform's direct SMGW ingest endpoint accepts.

        The body, not the whole call: the Marktlokation is a path parameter on
        every ingest endpoint this shape targets, so putting it in the body would
        model a field the endpoint does not read.

        A gap is an **absent** interval, never a zero: substitution
        (Ersatzwertbildung) is the path being tested, and a zero handed to a
        platform is a reading it settles against. A period the gateway delivered
        under a non-`MEASURED` quality is present *and* stamped, which is the
        other half — an Ersatzwert is billable and a `FAULTY` reading is not, and
        a platform has to tell them apart.

        Values are rendered as **decimal strings**. Energy is a decimal quantity
        and a JSON float carries a binary rounding error into whatever the
        platform settles against.

        Intervals run over the **Europe/Berlin** local day. The Gas day is a
        different window (06:00–06:00) and is not this shape.

        `session_id` is the idempotency key; it defaults to `<MeLo>-<Tag>`, which
        is stable across runs so a re-submission is recognised as one.
        """
        gaps = set(self.luecken)
        start, _ = berlin_day_bounds(self.tag)
        day = _dt.datetime.fromisoformat(start)
        step = _dt.timedelta(minutes=self.mtu_minutes)
        intervals: list[dict[str, object]] = []
        for i, value in enumerate(self.werte):
            if i in gaps:
                continue
            interval: dict[str, object] = {
                "from": (day + i * step).isoformat(),
                "to": (day + (i + 1) * step).isoformat(),
                "value": f"{round(Decimal(str(value)), 4):f}",
                "unit": "kWh",
            }
            if i in self.qualitaeten:
                interval["quality"] = self.qualitaeten[i]
            intervals.append(interval)
        return {
            "session_id": session_id or f"{self.melo_id}-{self.tag}",
            "source": "DIRECT_PUSH",
            "obis_code": obis_code,
            "melo_id": self.melo_id,
            "sender_mp_id": sender_mp_id,
            "intervals": intervals,
        }


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
        """Open a CLS channel, or fail if the gateway cannot carry one.

        Two conditions close the control path, and a §14a dispatch has to handle
        both before it steers:

        * a revoked or expired certificate — no session can be established;
        * a gateway not operated under :data:`STEUERUNG_TAF` — TAF-11 is the
          steering Tarifanwendungsfall, and a gateway ordered under TAF-14
          delivers measurements fast but cannot be steered at all. Ordering the
          wrong TAF is a real configuration error, and one whose symptom is a
          dispatch that silently does nothing.
        """
        state = self.certificate_state
        channel = self.channels.setdefault(channel_id, ClsChannel(channel_id))
        if state in ("expired", "revoked"):
            channel.open = False
            channel.last_error = f"certificate {state}"
        elif self.taf != STEUERUNG_TAF:
            channel.open = False
            channel.last_error = (
                f"gateway runs {self.taf} ({TAF_PROFILES[self.taf]}); steering "
                f"needs {STEUERUNG_TAF}"
            )
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
        werte: list[float] | None = None,
        luecken: list[int] | None = None,
        qualitaeten: dict[int, str] | None = None,
        mtu_minutes: int = 15,
        flat_kwh: float = 1.0,
    ) -> Zaehlerstandsgang:
        """Record a delivered series for `tag` (ISO 8601).

        `werte` must carry one value per market time unit of that Europe/Berlin
        day — 92, 96 or 100 at quarter-hourly resolution. Omit it for a flat
        series of `flat_kwh`, which is the right length by construction and is
        what most tests want.

        `luecken` are indices the gateway could not supply. A platform must form
        an Ersatzwert rather than treat a gap as zero, so the gap case has to be
        producible.

        `qualitaeten` stamps individual periods with a BDEW Messwertstatus (see
        :data:`READING_QUALITIES`). That is the other half of the same story: a
        `SUBSTITUTED` value is delivered *and* billable, a `FAULTY` one is
        delivered and must not be billed.
        """
        if self.certificate_state in ("expired", "revoked"):
            raise RuntimeError(
                f"gateway certificate is {self.certificate_state}; a real SMGW "
                f"would not deliver — assert on certificate_state before calling"
            )
        if werte is None:
            werte = [flat_kwh] * berlin_mtu_count(tag, mtu_minutes)
        gang = Zaehlerstandsgang(
            melo_id=self.melo_id,
            taf=self.taf,
            tag=tag,
            werte=list(werte),
            luecken=sorted(luecken or []),
            qualitaeten=dict(qualitaeten or {}),
            mtu_minutes=mtu_minutes,
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
