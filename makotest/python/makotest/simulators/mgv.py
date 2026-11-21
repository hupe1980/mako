"""The Marktgebietsverantwortliche.

Receives the aggregierte Mehr-/Mindermengen-Rechnung a Netzbetreiber sends it —
INVOIC 31007, and 31008 for the selbst ausgestellte Rechnung, both „NB an MGV"
(INVOIC AHB 1.0b Kap. 3.1.3) — and answers it in the three layers the Gas
documents give a receiver:

1. **CONTRL, always.** In the Sparte Gas the CONTRL is both the
   Empfangsbestätigung of a syntactically sound interchange and the
   Syntaxfehlermeldung of a broken one (CONTRL AHB 1.0 Kap. 2.3). The
   Syntaxprüfung holds the interchange against the BDEW-Nachrichtenbeschreibung
   — segments, codes, formats — and checks that it reached the right MP-ID
   (Kap. 2 and 2.1), so a MIG finding or an interchange addressed to someone
   else earns a negative CONTRL, UCI DE0083 `4` (Kap. 2.3.2), and nothing more:
   a rightly refused interchange counts as never received (APERAK AHB 1.1
   Kap. 1.4).
2. **APERAK, only on a Verarbeitbarkeitsfehler.** A Gas receiver reports the
   failing result alone and never sends an Anerkennungsmeldung (APERAK AHB 1.1
   Kap. 2.3). The `ERC` is the Kap. 4.3 code for the AHB finding — `Z29` a
   missing Angabe, `Z39` a code outside the Wertebereich, `Z40` an exceeded
   Wiederholbarkeit, `Z35` a broken Formatbedingung — and `Z31` for a
   Prüfidentifikator an MGV does not process. `Z10` and `Z18` are never used:
   Kap. 4.3 excludes both from the INVOIC.
3. **REMADV, the business answer** to every processable invoice (INVOIC AHB
   1.0b and REMADV AHB 1.0a, Kap. 1 each): 33001 Zahlungsavis, which transfers
   the fällige Betrag in full because invoices are settled „ganz oder gar
   nicht" (REMADV AHB 1.0a Kap. 3), or 33002 Abweisung, the only Abweisung
   PID open to Gas — 33003/33004 are Strom-only (Kap. 3.1.2). The refusal's
   `AJT` carries a code from Codeliste `G_0080`, the list EBD `E_0802`
   „aggregierte MMM-Rechnung prüfen" publishes (Entscheidungsbaum-Diagramme und
   Codelisten 4.3 Kap. 11.3.1), and DE 1082 names `G_0080`, the value the
   REMADV AHB 1.0a admits for a Gas recipient. Each answered invoice gets its
   own REMADV interchange, because a REMADV-Datei carries exactly one
   REMADV-Nachricht (REMADV AHB 1.0a Kap. 3, Fußnote 1).

It refuses anything not addressed to an MGV: an interchange for another MP-ID
with the negative CONTRL, a message an MGV does not process with APERAK `Z31`.
A simulator that paid a Netznutzungsrechnung as an MMM-Rechnung would agree to
something no real counterparty does, and every assertion downstream of that
payment would be meaningless.

What it leaves out, because no document read for it states the answer:

- **ALOCAT.** The NB's allocation messages to the MGV are DVGW EDIG@S formats,
  and makotest's core validates only the BDEW EDIFACT family — an ALOCAT is
  refused with an error rather than acknowledged unread.
- **Timing.** The CONTRL is due within 6 hours (CONTRL AHB 1.0 Kap. 2.3.1); the
  APERAK window depends on whether a Geschäftsvorfall is an Initial- or a
  Folgeprozess (APERAK AHB 1.1 Kap. 2.3.1), which no AHB settles for the
  MMM-Rechnung; the REMADV AHB names no answer window at all. The simulator
  answers at once and models no lateness.
- **Stornorechnungen** and their tree `E_0806`, because 31007/31008 are the
  invoices themselves.
- **Code `28`** „Sonstiges": the REMADV AHB 1.0a makes `SG7 FTX` Muss beside
  it (condition [2]), and the REMADV builder writes no Kopf-level FTX.
- **The Fehlerort of a negative CONTRL.** `build_contrl_for` rejects at `UCI`
  level with syntax error `12` and names no segment, so the negative CONTRL
  says *that* the interchange was refused, not where; the CONTRL AHB requires
  the location, and the platform's own AHB check refuses it for that.
"""

from __future__ import annotations

from dataclasses import dataclass

from .._native import (
    build_aperak_for,
    build_contrl_for,
    build_interchange,
    build_remadv,
    validate_edifact,
)
from .marktpartner import _wire_text

__all__ = [
    "G_0080",
    "MGV_INVOIC_PIDS",
    "MgvReply",
    "MgvSim",
    "MmmAblehnung",
    "Rechnungseingang",
]

#: The INVOIC Prüfidentifikatoren addressed to a Marktgebietsverantwortlichen:
#: the aggregierte MMM-Rechnung 31007 and its selbst ausgestellte twin 31008,
#: the only two INVOIC use cases whose Kommunikation is „NB an MGV" (INVOIC AHB
#: 1.0b Kap. 3.1.3). The Kapazitätsrechnung 31010 goes to the KN.
MGV_INVOIC_PIDS = frozenset({31007, 31008})

#: Codeliste `G_0080` „aggregierte Menge und Abrechnungszeitraum prüfen", the
#: codes an MGV refuses an aggregierte MMM-Rechnung with (EBD `E_0802`,
#: Entscheidungsbaum-Diagramme und Codelisten 4.3 Kap. 11.3.1.1).
G_0080: dict[str, str] = {
    "5": "Preis/Rechenregel falsch",
    "9": "Falscher Abrechnungszeitraum (innerhalb gültiger Vertragsgrenzen)",
    "14": "Unbekannte Marktlokation, Messlokation",
    "28": "Sonstiges",
    "53": "doppelte Rechnung",
    "Z01": "Abrechnungsbeginn ungleich Vertragsbeginn",
    "Z02": "Abrechnungsende ungleich Vertragsende",
    "Z06": "Artikel nicht vereinbart",
    "Z07": "Netznutzungsmesswerte / -energiemengen fehlen",
    "Z08": "Rechnungsnummer bereits erhalten",
    "Z10": "Netznutzungsmesswerte / -energiemengen falsch",
    "Z35": "falscher Bilanzierungsbeginn",
    "Z36": "falsches Netznutzungsende",
    "Z37": "bilanzierte Menge fehlt",
    "Z38": "bilanzierte Menge falsch",
    "Z39": "Netznutzungsabrechnung fehlt",
    "Z40": "Reverse Charge Anwendung fehlt oder unzulässig",
    "Z41": "Allokationsliste fehlt",
    "Z42": "Mehr-/Mindermenge falsch",
    "Z43": "Ungültiges Rechnungsdatum",
    "Z44": "Zeitintervall der bilanzierten Menge inkonsistent",
    "Z45": (
        "Rechnungsempfänger widerspricht der steuerrechtlichen Einschätzung "
        "des Rechnungsstellers"
    ),
}

#: Validation layers the CONTRL Syntaxprüfung covers: the UN/EDIFACT syntax and
#: the BDEW-Nachrichtenbeschreibung, the MIG (CONTRL AHB 1.0 Kap. 2). What the
#: AHB adds per Prüfidentifikator is the APERAK's (APERAK AHB 1.1 Kap. 2.1.2).
_CONTRL_LAYERS = frozenset({"parse", "directory", "mig"})

#: The two REMADV use cases open to a Gas recipient: Bestätigung and Abweisung
#: (REMADV AHB 1.0a Kap. 3.1.1).
ZAHLUNGSAVIS_PID = 33001
ABWEISUNG_PID = 33002

#: APERAK AHB 1.1 Kap. 4.3 — the AHB-Fehler code for each kind of AHB finding
#: the validator reports, keyed on the suffix of its rule id.
_AHB_FEHLERCODES = (
    ("-MISSING", "Z29"),  # Erforderliche Angabe für diesen Anwendungsfall fehlt
    ("-CODE", "Z39"),  # Code nicht aus erlaubtem Wertebereich
    ("-MAX", "Z40"),  # Segment- bzw. Segmentgruppenwiederholbarkeit überschritten
)
#: An AHB Formatbedingung (`[UB2]`, `[931]` …): Z35, Format nicht eingehalten.
_FORMATBEDINGUNG = "FMT-"


@dataclass(frozen=True)
class MmmAblehnung:
    """A refusal of an aggregierte MMM-Rechnung, drawn from `G_0080`."""

    code: str

    def __post_init__(self) -> None:
        if self.code not in G_0080:
            raise ValueError(
                f"{self.code!r} is not in Codeliste G_0080 — an MGV refuses an "
                f"aggregierte MMM-Rechnung only with {sorted(G_0080)}"
            )
        if self.code == "28":
            raise ValueError(
                "code 28 obliges an SG7 FTX Erläuterung (REMADV AHB 1.0a, "
                "condition [2]) that the REMADV builder does not write"
            )

    @property
    def name(self) -> str:
        """The code's name in `G_0080`."""
        return G_0080[self.code]


@dataclass
class Rechnungseingang:
    """One invoice message the simulator received."""

    pid: int | None
    #: `BGM` DE 1004 of the invoice, when it was read.
    rechnungsnummer: str | None
    valid: bool
    #: `True` when a Zahlungsavis 33001 was sent.
    accepted: bool
    ablehnung: MmmAblehnung | None = None
    #: The APERAK `ERC`, when the message was not processable.
    erc: str | None = None
    #: `False` when the PID had no AHB rules, so `valid` was decided vacuously.
    checked: bool = True
    #: Why the message was refused outright, when it was not an MGV's to process.
    refused: str | None = None


@dataclass(frozen=True)
class MgvReply:
    """What the MGV put on the wire in response to one interchange.

    Every field holding bytes is a rendered **interchange**, ready to feed back
    to the platform under test.
    """

    #: The CONTRL interchange — positive or negative, always present.
    contrl: bytes
    contrl_positive: bool
    #: One interchange carrying one APERAK per unprocessable message, or `None`.
    aperak: bytes | None = None
    #: The APERAK `ERC` codes, one per refused message.
    ercs: tuple[str, ...] = ()
    #: One REMADV interchange per processable invoice, in request order.
    remadvs: tuple[bytes, ...] = ()
    #: The REMADV Prüfidentifikator of each, aligned with `remadvs`.
    remadv_pids: tuple[int, ...] = ()
    #: The `G_0080` code of each REMADV, `None` on a Zahlungsavis.
    antwort_codes: tuple[str | None, ...] = ()


class MgvSim:
    """A Marktgebietsverantwortlicher that pays or refuses MMM-Rechnungen.

    ::

        mgv = MgvSim(mp_id="9879999000004", reference_date="2026-04-01")
        mgv.reject_next(MmmAblehnung("Z42"))
        first  = mgv.receive(invoic_wire)   # REMADV 33002, AJT+Z42+G_0080
        second = mgv.receive(invoic_wire)   # REMADV 33001 — the queue drained
    """

    def __init__(self, *, mp_id: str, reference_date: str) -> None:
        self.mp_id = mp_id
        self.reference_date = reference_date
        self._sent = 0
        self.rechnungen: list[Rechnungseingang] = []
        self._pending: list[MmmAblehnung] = []

    # ── Configuration ─────────────────────────────────────────────────────────

    def reject_next(self, ablehnung: MmmAblehnung) -> MgvSim:
        """Refuse the next processable invoice with `ablehnung`, then pay again.

        Queued rather than sticky, so a test can assert that the corrected
        invoice after a refusal is paid.
        """
        self._pending.append(ablehnung)
        return self

    # ── Behaviour ─────────────────────────────────────────────────────────────

    def receive(self, raw: bytes) -> MgvReply:
        """Handle one inbound interchange and answer it."""
        if not raw.lstrip().startswith((b"UNA", b"UNB")):
            raise ValueError(
                "a Marktgebietsverantwortlicher receives interchanges, not bare "
                "messages — wrap the message with build_interchange() first"
            )
        report = validate_edifact(raw, self.reference_date)
        if not report.messages:
            raise ValueError("the interchange carries no message to answer")
        for message in report.messages:
            if message.message_type is None:
                raise ValueError(
                    "the interchange carries a message type makotest's core does "
                    "not validate — an ALOCAT is a DVGW EDIG@S format, and "
                    "acknowledging it unread would assert nothing"
                )
        sender = report.envelope.sender_id if report.envelope else self.mp_id

        syntax = any(f.rule_origin in _CONTRL_LAYERS for f in report.errors)
        envelope = report.envelope
        broken = envelope is not None and not envelope.is_structurally_valid
        elsewhere = envelope is not None and envelope.receiver_id != self.mp_id
        if syntax or broken or elsewhere:
            return MgvReply(
                contrl=self._wrap([self._contrl(raw, accept=False)], to=sender),
                contrl_positive=False,
            )

        contrl = self._wrap([self._contrl(raw, accept=True)], to=sender)
        aperaks: list[bytes] = []
        ercs: list[str] = []
        remadvs: list[bytes] = []
        remadv_pids: list[int] = []
        codes: list[str | None] = []
        segments = _messages(raw)
        for message in report.messages:
            erc, text, eingang = self._assess(message, segments[message.index])
            self.rechnungen.append(eingang)
            if erc is not None:
                aperaks.append(
                    build_aperak_for(
                        raw,
                        on=self.reference_date,
                        error_code=erc,
                        error_text=text,
                        message_ref=str(len(aperaks) + 1),
                        message_index=message.index,
                    )
                )
                ercs.append(erc)
                continue
            pid, remadv = self._remadv(segments[message.index], eingang, to=sender)
            remadvs.append(self._wrap([remadv], to=sender))
            remadv_pids.append(pid)
            codes.append(eingang.ablehnung.code if eingang.ablehnung else None)

        return MgvReply(
            contrl=contrl,
            contrl_positive=True,
            aperak=self._wrap(aperaks, to=sender) if aperaks else None,
            ercs=tuple(ercs),
            remadvs=tuple(remadvs),
            remadv_pids=tuple(remadv_pids),
            antwort_codes=tuple(codes),
        )

    def _assess(
        self, message, segments: list[list[list[str]]]
    ) -> tuple[str | None, str | None, Rechnungseingang]:
        """Decide one message: an `(ERC, text)` refusal, or `(None, None)`."""
        pid = message.pruefidentifikator
        nummer = _value(segments, "BGM", 2)
        checked = message.rules_applied
        # Z31 comes before every other check: „In diesem Fall wird keine weitere
        # Prüfung des Geschäftsvorfalls durchgeführt" (APERAK AHB 1.1 Kap. 4.3).
        if message.message_type != "INVOIC" or pid not in MGV_INVOIC_PIDS:
            reason = (
                f"{message.message_type} {pid} is not an invoice addressed to a "
                "Marktgebietsverantwortlichen"
            )
            eingang = Rechnungseingang(
                pid,
                nummer,
                message.is_valid,
                False,
                erc="Z31",
                checked=checked,
                refused=reason,
            )
            return "Z31", _wire_text(reason), eingang
        if not message.is_valid:
            finding, erc = _ahb_fehler(message)
            eingang = Rechnungseingang(
                pid, nummer, False, False, erc=erc, checked=checked
            )
            return erc, _wire_text(finding.message), eingang
        ablehnung = self._pending.pop(0) if self._pending else None
        eingang = Rechnungseingang(
            pid, nummer, True, ablehnung is None, ablehnung, checked=checked
        )
        return None, None, eingang

    def _remadv(
        self, segments: list[list[list[str]]], eingang: Rechnungseingang, *, to: str
    ) -> tuple[int, bytes]:
        """The REMADV answering one processable invoice."""
        betrag = _faelliger_betrag(segments)
        datum = _value(segments, "DTM", 1, 1, qualifier="137")
        if betrag is None or datum is None or eingang.rechnungsnummer is None:
            raise ValueError(
                "the invoice lacks its BGM number, DTM+137 or SG50 MOA+9, which "
                "the AHB makes Muss and the REMADV has to echo"
            )
        pid = ZAHLUNGSAVIS_PID if eingang.ablehnung is None else ABWEISUNG_PID
        remadv = build_remadv(
            pid,
            self.mp_id,
            to,
            rechnungsnummer=eingang.rechnungsnummer,
            faelliger_betrag=betrag,
            # An Abweisung transfers nothing; a Zahlungsavis the whole amount,
            # because no invoice is part-paid (REMADV AHB 1.0a Kap. 3).
            ueberweisungsbetrag="0" if eingang.ablehnung else betrag,
            # DTM 2379 `303` is CCYYMMDDHHMMZZZ; the REMADV writes its own `+00`.
            rechnungsdatum=datum[:12],
            dokumentenart=_value(segments, "BGM", 1) or "380",
            on=self.reference_date,
            kopf_gruende=(
                [(eingang.ablehnung.code, "G_0080")] if eingang.ablehnung else None
            ),
        )
        return pid, remadv

    def _contrl(self, raw: bytes, *, accept: bool) -> bytes:
        return build_contrl_for(raw, on=self.reference_date, accept=accept)

    def _wrap(self, messages: list[bytes], *, to: str) -> bytes:
        # UNB DE0020 identifies the interchange to the receiver; a counter
        # rather than a clock keeps a scenario byte-reproducible.
        self._sent += 1
        return build_interchange(
            sender=self.mp_id,
            receiver=to,
            dar=f"MG{self._sent:06d}",
            messages=messages,
            on=self.reference_date,
        )

    # ── Inspection ────────────────────────────────────────────────────────────

    @property
    def paid(self) -> list[Rechnungseingang]:
        """Invoices answered with a Zahlungsavis."""
        return [r for r in self.rechnungen if r.accepted]

    @property
    def abgewiesen(self) -> list[Rechnungseingang]:
        """Invoices answered with a REMADV Abweisung."""
        return [r for r in self.rechnungen if r.ablehnung is not None]

    @property
    def unvalidated_rechnungen(self) -> list[int]:
        """PIDs received whose AHB rules were never applied."""
        return [r.pid for r in self.rechnungen if r.pid is not None and not r.checked]

    def __repr__(self) -> str:
        return (
            f"MgvSim(mp_id={self.mp_id!r}, rechnungen={len(self.rechnungen)}, "
            f"abgewiesen={len(self.abgewiesen)})"
        )


def _ahb_fehler(message):
    """The first AHB finding of `message` and its APERAK AHB 1.1 Kap. 4.3 code.

    Only AHB findings reach here — a MIG finding has already earned the
    negative CONTRL. A finding no Kap. 4.3 code names is refused rather than
    answered with a guessed one.
    """
    for finding in message.errors:
        if finding.rule_id is None:
            continue
        if finding.rule_id.startswith(_FORMATBEDINGUNG):
            return finding, "Z35"
        if finding.rule_origin != "ahb":
            continue
        for suffix, code in _AHB_FEHLERCODES:
            if finding.rule_id.endswith(suffix):
                return finding, code
    raise ValueError(
        "the invoice is not processable, and none of its findings is an AHB "
        "finding APERAK AHB 1.1 Kap. 4.3 gives a code for: "
        f"{[f.rule_id for f in message.errors]}"
    )


def _messages(raw: bytes) -> list[list[list[list[str]]]]:
    """Split an interchange into messages, each a list of segments of elements.

    Each element is its list of components. The service characters come from
    `UNA` when present, else the ISO 9735 defaults.
    """
    text = raw.decode("latin-1").lstrip()
    component, element, release, terminator = ":", "+", "?", "'"
    if text.startswith("UNA"):
        component, element, release, terminator = text[3], text[4], text[6], text[8]
        text = text[9:]
    messages: list[list[list[list[str]]]] = []
    for segment in _split(text, terminator, release):
        elements = [
            [_unescape(c, release) for c in _split(e, component, release)]
            for e in _split(segment.strip(), element, release)
        ]
        tag = elements[0][0] if elements and elements[0] else ""
        if tag == "UNH":
            messages.append([])
        if messages and tag not in ("UNZ",):
            messages[-1].append(elements)
    return messages


def _split(text: str, separator: str, release: str) -> list[str]:
    parts, current, escaped = [], [], False
    for c in text:
        if escaped:
            current.append(c)
            escaped = False
        elif c == release:
            current.append(c)
            escaped = True
        elif c == separator:
            parts.append("".join(current))
            current = []
        else:
            current.append(c)
    if current:
        parts.append("".join(current))
    return parts


def _unescape(value: str, release: str) -> str:
    out, escaped = [], False
    for c in value:
        if c == release and not escaped:
            escaped = True
            continue
        out.append(c)
        escaped = False
    return "".join(out)


def _value(
    segments: list[list[list[str]]],
    tag: str,
    element: int,
    component: int = 0,
    *,
    qualifier: str | None = None,
) -> str | None:
    """A component of the first `tag` segment, data elements counted from 1.

    With `qualifier`, only a segment whose first component carries it counts —
    `DTM+137`, not the first `DTM`.
    """
    for segment in segments:
        if segment[0][0] != tag:
            continue
        if qualifier is not None and (len(segment) < 2 or segment[1][0] != qualifier):
            continue
        if element < len(segment) and component < len(segment[element]):
            if segment[element][component]:
                return segment[element][component]
    return None


def _faelliger_betrag(segments: list[list[list[str]]]) -> str | None:
    """`SG50 MOA+9` — the fällige Betrag, the first `MOA+9` after `UNS`."""
    after_uns = False
    for segment in segments:
        tag = segment[0][0]
        if tag == "UNS":
            after_uns = True
        elif after_uns and tag == "MOA" and segment[1][0] == "9":
            return segment[1][1]
    return None
