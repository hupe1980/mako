"""The opposing Marktpartner.

Receives an interchange, acknowledges it, then answers with the AHB response for
that Prüfidentifikator — a Bestätigung, an Ablehnung, or nothing at all.

Six things make it more than a stub.

**It answers in EDIFACT**, built by the same Rust builders the platform uses,
with the parties mirrored and the request's SG4 object and references echoed. A
dictionary answer could not be fed back into the system under test.

**It picks the right acknowledgement.** A CONTRL reports a *syntax* failure and
an APERAK an *application* one — different messages, telling the counterparty to
retry different things. The choice comes from the validation layer that fired.

**It answers every message it was sent.** An interchange routinely carries
several, each a separate Vorgang with its own Prüfidentifikator; answering only
the first and calling that the answer is how a broken second one ships.

**It states an Antwortcode the Entscheidungsbaum publishes.** `SG4 STS+E01` is
AHB-Muss on every Antwortnachricht. Code and Codeliste are resolved from the
tree the answer-Frist table names, so a code that tree has no leaf for is
refused at binding time rather than by the counterparty.

**It remembers what it accepted.** A repeat Anmeldung for a Lokation the
counterparty already holds meets `E_0622`'s `A06`, not the first answer again —
see `Vorgangsregister` and `.on(pid).bei_offenem_vorgang()`.

**It can misbehave on purpose.** `.timeout()` sends nothing at all, which is the
only way to test the platform's own Fristen. `.antwort(pid=…)` answers with a PID
the AHB does not assign. `delay_werktage=` answers after the Frist has expired.
`strict=False` answers a request it should have refused. An unconfigured partner
acknowledges but sends no business answer.

It is a plain object with `receive()`, not a server, so an AS4 transport layers
on top instead of being a dependency of everyone who does not need one.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Literal

from .._native import (
    ablehnung_pid,
    answer_pids,
    antwort_code,
    antwort_codes,
    antwort_obligation,
    bestaetigung_pid,
    build_answer,
    build_aperak_for,
    build_contrl_for,
    build_interchange,
    deadline_at_werktage,
    validate_edifact,
)

__all__ = [
    "Answer",
    "AntwortModus",
    "Exchange",
    "MarktpartnerSim",
    "Reply",
    "Rulebook",
    "ablehnung_pid",
    "answer_pids",
    "bestaetigung_pid",
]

AntwortModus = Literal["bestaetigung", "ablehnung", "timeout"]


def _wire_text(text: str | None, *, limit: int = 70) -> str | None:
    """Trim derived prose to what the interchange charset can carry.

    The simulator *composes* this text from a validation finding rather than
    taking it from the caller, so it owns the result's encodability: an
    interchange declares `UNB+UNOC:3` (ISO 8859-1), and a finding message may
    carry an em-dash or `∈`. Dropping the characters that cannot travel keeps a
    refusal from failing on its own explanation — a caller-supplied text is
    refused instead, where naming the problem is the useful answer.
    """
    if text is None:
        return None
    return "".join(c for c in text if ord(c) <= 0xFF)[:limit] or None


#: Validation layers whose findings are a *syntax* problem, answered with a
#: CONTRL. Everything else — MIG structure, AHB conditions, semantic rules — is
#: an application problem and earns an APERAK.
_SYNTAX_LAYERS = frozenset({"parse", "directory"})


@dataclass(frozen=True)
class Answer:
    """What the simulator will send back for one Prüfidentifikator."""

    modus: AntwortModus
    #: Answer PID. `None` for `timeout`.
    pid: int | None = None
    #: `SG4 STS+E01` DE 9013 — the Antwortcode, e.g. `"A06"`. Written into the
    #: message and reported on the `Reply`.
    #:
    #: Not an ERC: `ERC` is the APERAK/CONTRL segment for processability errors,
    #: and an Antwortcode is a business answer with its own Codeliste.
    antwort_code: str | None = None
    #: The Entscheidungsbaum `antwort_code` was resolved against, e.g.
    #: `"E_0622"`. What goes into DE 1131 is derived from it — see
    #: `Answer.wire_codeliste`.
    ebd: str | None = None
    #: `SG4 STS+E01` DE 1131 — the Codeliste the answer names on the wire. The
    #: EBD number for a GPKE or GeLi Gas tree, an `S_xxxx` / `G_xxxx` for a WiM
    #: one, and `None` where the answer names no list.
    wire_codeliste: str | None = None
    #: `(qualifier, YYYYMMDD)` SG4 DTM pairs for the answer transaction, e.g.
    #: `("92", "20261101")` for a confirmed Zuordnungsbeginn.
    process_dates: tuple[tuple[str, str], ...] = ()
    #: Extra `(qualifier, value)` RFF pairs for the answer transaction.
    references: tuple[tuple[str, str], ...] = ()
    #: `(text function, text)` FTX pairs — `("ACB", …)` is the Erläuterung
    #: several Antwortcodes are incomplete without.
    free_texts: tuple[tuple[str, str], ...] = ()
    #: When set, this binding applies **only** to a request whose Lokation the
    #: counterparty already holds an open Vorgang for. The unconditional binding
    #: for the same PID answers the first request; this one answers the repeat.
    nur_bei_offenem_vorgang: bool = False
    #: Werktage to sit on the answer before sending it. `0` answers the moment
    #: the request arrives; anything else sends at **17:00 Berlin on the n-th
    #: Werktag**, which is how a *late* answer is produced — the message is
    #: conformant and the timing is not, and a platform has to notice. It moves
    #: `Reply.answered_at`, never `Reply.due_at`: the Frist is the Festlegung's
    #: and a counterparty does not get to move it.
    delay_werktage: int = 0


@dataclass(frozen=True)
class Reply:
    """What the counterparty put on the wire in response to one interchange.

    Both `ack` and `business` are rendered **interchanges** — feed them straight
    back to the platform under test.
    """

    #: The CONTRL or APERAK interchange, or `None` when the partner stayed
    #: silent (`.timeout()`).
    ack: bytes | None = None
    #: `"CONTRL"` for a syntax outcome, `"APERAK"` for an application one.
    ack_kind: Literal["CONTRL", "APERAK"] | None = None
    #: `True` when the acknowledgement is positive.
    ack_positive: bool = True
    #: The business-answer interchange, or `None` when there is none. It carries
    #: one message per answered request message, in request order.
    business: bytes | None = None
    #: The answer Prüfidentifikatoren carried by `business`, in that order.
    pids: tuple[int, ...] = ()
    #: The Antwortmodus per answered message, aligned with `pids`.
    modi: tuple[AntwortModus, ...] = ()
    #: The `SG4 STS+E01` Antwortcodes per answered message, aligned with `pids`.
    #:
    #: A **business** answer — why the counterparty decided as it did. Distinct
    #: from `ercs`, which is the acknowledgement's processability verdict: an
    #: APERAK `Z10` says the message could not be processed, an Antwortcode
    #: `A06` says it was processed and refused.
    antwort_codes: tuple[str | None, ...] = ()
    #: The APERAK `ERC` codes the acknowledgement carries, one per acknowledged
    #: message. Empty for a positive acknowledgement and for a CONTRL.
    ercs: tuple[str | None, ...] = ()
    #: When each answered message was due, from the platform's own Frist table,
    #: aligned with `pids`. An entry is `None` when no Festlegung quantifies that
    #: process's window, or when `receive()` was called without `received_at`.
    #:
    #: Per message, because the window is a property of the *request*: an
    #: interchange carrying a 55001 and a 55004 owes two answers on two different
    #: clocks, and one instant would be wrong for one of them.
    due_ats: tuple[str | None, ...] = ()
    #: When the partner sent the answer, once `received_at` was supplied. One
    #: value, because every answer rides one interchange and so leaves at one
    #: instant. Later than `due_at` when the binding asked for a delay — which is
    #: the whole point of being able to ask.
    answered_at: str | None = None

    def __bool__(self) -> bool:
        """Truthy when the partner said anything at all."""
        return self.ack is not None or self.business is not None

    @property
    def pid(self) -> int | None:
        """The answer PID, for the single-message case.

        Raises when the reply answers several messages: one PID cannot speak for
        two Vorgänge, and returning the first would be wrong for the rest. Read
        `pids` there.
        """
        return _only("pid", self.pids)

    @property
    def modus(self) -> AntwortModus | None:
        """The Antwortmodus, for the single-message case."""
        return _only("modus", self.modi)

    @property
    def antwort_code(self) -> str | None:
        """The `SG4 STS+E01` Antwortcode, for the single-message case."""
        return _only("antwort_code", self.antwort_codes)

    @property
    def erc(self) -> str | None:
        """The APERAK `ERC`, for the single-acknowledgement case."""
        return _only("erc", self.ercs)

    @property
    def due_at(self) -> str | None:
        """When the answer was due, for the single-message case."""
        return _only("due_at", self.due_ats)


def _only(name: str, values: tuple[object, ...]):
    if len(values) > 1:
        raise ValueError(
            f"this reply answers {len(values)} messages — read `{name}s`, because "
            f"one value would be wrong for all but one of them"
        )
    return values[0] if values else None


class Rulebook:
    """Fluent binding of one request PID to its answer.

    Returned by :meth:`MarktpartnerSim.on`; each terminal call registers the
    answer and returns the simulator so calls can be chained.
    """

    def __init__(
        self, sim: MarktpartnerSim, pid: int, *, nur_bei_offenem_vorgang: bool = False
    ) -> None:
        self._sim = sim
        self._pid = pid
        self._nur_bei_offenem_vorgang = nur_bei_offenem_vorgang

    def bei_offenem_vorgang(self) -> Rulebook:
        """Bind the answer the counterparty gives a **repeat** request.

        A Netzbetreiber holding an open Vorgang for a Marktlokation does not
        answer a second Anmeldung for it the way it answered the first —
        `E_0622` publishes `A06` „Andere Anmeldung in Bearbeitung" for exactly
        that::

            nb.on(55001).bestaetigung(antwort_code="A51", ebd="E_0623")
            nb.on(55001).bei_offenem_vorgang().ablehnung(
                antwort_code="A06", process_dates=[("Z07", "20260501")]
            )

        The unconditional binding answers the first request and opens the
        Vorgang; this one answers every request that meets it still open. Close
        it with `sim.vorgaenge.schliessen(lokation)`.
        """
        return Rulebook(self._sim, self._pid, nur_bei_offenem_vorgang=True)

    def bestaetigung(
        self,
        *,
        antwort_code: str | None = None,
        ebd: str | None = None,
        bemerkung: str | None = None,
        process_dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
        delay_werktage: int = 0,
    ) -> MarktpartnerSim:
        """Answer with the AHB Bestätigung for this PID.

        `antwort_code` is the `SG4 STS+E01` code the answer states, resolved
        against `ebd` — which defaults to the Entscheidungsbaum the answer-Frist
        table names for this request. A GPKE Anmeldung is decided by two trees
        in sequence and the table names the first, so confirming a 55001 means
        naming `ebd="E_0623"` explicitly: `E_0622` is the Vorprüfung and
        publishes refusals only.
        """
        pid = bestaetigung_pid(self._pid)
        if pid is None:
            raise ValueError(
                f"PID {self._pid} has no Bestätigung in the AHB answer table. "
                f"Either it is not a request PID, or the pair is incomplete — "
                f"send an explicit answer with .antwort(pid=...)."
            )
        return self._bind(
            Answer(
                "bestaetigung",
                pid=pid,
                process_dates=tuple(process_dates or ()),
                references=tuple(references or ()),
                free_texts=(("ACB", bemerkung),) if bemerkung else (),
                delay_werktage=delay_werktage,
                **self._resolve_code(
                    antwort_code, ebd, accepted=True, bemerkung=bemerkung
                ),
            )
        )

    def ablehnung(
        self,
        *,
        antwort_code: str | None = None,
        ebd: str | None = None,
        bemerkung: str | None = None,
        process_dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
        delay_werktage: int = 0,
    ) -> MarktpartnerSim:
        """Answer with the AHB Ablehnung, stating why.

        `antwort_code` is the `SG4 STS+E01` code, resolved against `ebd` — which
        defaults to the Entscheidungsbaum the answer-Frist table names for this
        request. A code that tree does not publish raises here rather than
        travelling to the platform under test as a plausible-looking answer no
        Entscheidungsbaum has a leaf for.

        A code routinely makes another segment conditional-Muss. `bemerkung`
        writes the `FTX+ACB` Erläuterung the BDEW demands beside some codes, and
        is required whenever the catalogue says so; `process_dates` carries the
        `SG4 DTM` a code obliges — `A06` („Andere Anmeldung in Bearbeitung")
        needs a `("Z07", …)`.
        """
        pid = ablehnung_pid(self._pid)
        if pid is None:
            raise ValueError(
                f"PID {self._pid} has no Ablehnung in the AHB answer table — "
                f"GeLi Gas 44020, for one, is confirmable but never rejectable. "
                f"Send an explicit answer with .antwort(pid=...)."
            )
        return self._bind(
            Answer(
                "ablehnung",
                pid=pid,
                process_dates=tuple(process_dates or ()),
                references=tuple(references or ()),
                free_texts=(("ACB", bemerkung),) if bemerkung else (),
                delay_werktage=delay_werktage,
                **self._resolve_code(
                    antwort_code, ebd, accepted=False, bemerkung=bemerkung
                ),
            )
        )

    def timeout(self) -> MarktpartnerSim:
        """Send nothing — not even an acknowledgement.

        This is how a Frist is tested: the platform must escalate on its own
        deadline rather than waiting for a peer that never answers.
        """
        return self._bind(Answer("timeout"))

    def antwort(
        self,
        *,
        pid: int,
        modus: AntwortModus = "bestaetigung",
        antwort_code: str | None = None,
        ebd: str | None = None,
        wire_codeliste: str | None = None,
        process_dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
        delay_werktage: int = 0,
    ) -> MarktpartnerSim:
        """Answer with an explicit PID, bypassing the answer table.

        For adversarial cases — answering with the wrong PID is a thing real
        counterparties do, and a platform should reject it.

        `antwort_code` is written unchecked here, which is the point: passing
        `wire_codeliste` too puts an arbitrary DE 9013 / DE 1131 pair on the
        wire, so a platform can be tested against a code its Entscheidungsbaum
        never publishes. Use `.bestaetigung()` / `.ablehnung()` for the
        conformant case, where the pair is resolved and checked.
        """
        if modus == "timeout":
            raise ValueError(
                "a timeout carries no PID — use .timeout(), or the binding says "
                "both 'answer with this' and 'answer with nothing'"
            )
        return self._bind(
            Answer(
                modus,
                pid=pid,
                antwort_code=antwort_code,
                ebd=ebd,
                wire_codeliste=wire_codeliste or ebd,
                process_dates=tuple(process_dates or ()),
                references=tuple(references or ()),
                delay_werktage=delay_werktage,
            )
        )

    def _resolve_code(
        self,
        code: str | None,
        ebd: str | None,
        *,
        accepted: bool,
        bemerkung: str | None = None,
    ) -> dict[str, str | None]:
        """Resolve an Antwortcode against its tree, or refuse the binding.

        Returns the three wire fields. A `ValueError` here means the *test* named
        a code the Entscheidungsbaum has no leaf for, or one whose Cluster
        contradicts the answer PID it would ride — both of which are defects in
        the binding rather than in the platform under test.
        """
        tree = ebd or self._default_ebd()
        if code is None:
            return {"antwort_code": None, "ebd": tree, "wire_codeliste": None}
        if tree is None:
            raise ValueError(
                f"no Entscheidungsbaum is published for request PID {self._pid}, "
                f"so {code!r} cannot be resolved — name one with ebd=, or send it "
                f"unchecked with .antwort(antwort_code=..., wire_codeliste=...)."
            )
        resolved = antwort_code(tree, code)
        if resolved is None:
            published = ", ".join(c.code for c in antwort_codes(tree))
            raise ValueError(
                f"{tree} does not publish Antwortcode {code!r}. A code means "
                f"nothing outside its tree. {tree} publishes: {published}."
            )
        if resolved.ist_zustimmung not in (None, accepted):
            wanted = "Bestätigung" if accepted else "Ablehnung"
            raise ValueError(
                f"{tree} classes {code!r} as {resolved.cluster}, so it cannot "
                f"ride the {wanted} PID: „{resolved.bedeutung}"
            )
        if resolved.braucht_bemerkung and not bemerkung:
            raise ValueError(
                f"{tree} requires a written Erläuterung beside {code!r} "
                f"(„{resolved.bedeutung}), so an answer without one is "
                f"incomplete. Pass bemerkung=."
            )
        return {
            "antwort_code": resolved.code,
            "ebd": tree,
            "wire_codeliste": resolved.wire_codeliste,
        }

    def _default_ebd(self) -> str | None:
        obligation = antwort_obligation(self._pid)
        return obligation.ebd if obligation else None

    def _bind(self, answer: Answer) -> MarktpartnerSim:
        return self._sim._register(
            self._pid,
            replace(answer, nur_bei_offenem_vorgang=self._nur_bei_offenem_vorgang),
        )


@dataclass(frozen=True)
class OffenerVorgang:
    """A Vorgang this counterparty has accepted and not yet closed."""

    #: The Lokation the Vorgang runs against — a MaLo or a MeLo.
    lokation: str
    #: `IDE+24` DE 7402 of the request that opened it.
    vorgangsnummer: str | None
    #: The request Prüfidentifikator that opened it.
    pid: int
    #: The answer PID the counterparty sent.
    antwort_pid: int | None


class Vorgangsregister:
    """What the counterparty has already accepted, keyed by Lokation.

    A real Netzbetreiber does not answer two Anmeldungen for one Marktlokation
    the same way: the second meets a Vorgang already in Bearbeitung, and
    `E_0622` publishes `A06` for exactly that. Without this memory the simulator
    answers both identically, and a platform that never re-checks its own
    outbound duplicates passes.

    Keyed on the **Lokation** rather than the Vorgangsnummer, because the
    Vorgangsnummer is the *sender's* reference and a duplicate request carries a
    new one — keying on it would make every duplicate look like a first request,
    which is the bug this exists to expose.
    """

    def __init__(self) -> None:
        self._offen: dict[str, OffenerVorgang] = {}

    def offen(self, lokation: str) -> OffenerVorgang | None:
        """The open Vorgang for `lokation`, if the counterparty holds one."""
        return self._offen.get(lokation)

    def eroeffnen(self, vorgang: OffenerVorgang) -> None:
        self._offen[vorgang.lokation] = vorgang

    def schliessen(self, lokation: str) -> OffenerVorgang | None:
        """Close the Vorgang for `lokation` — a Storno or a completed process."""
        return self._offen.pop(lokation, None)

    @property
    def offene(self) -> list[OffenerVorgang]:
        """Every Vorgang still open, in insertion order."""
        return list(self._offen.values())

    def __len__(self) -> int:
        return len(self._offen)

    def __repr__(self) -> str:
        return f"Vorgangsregister(offen={sorted(self._offen)})"


@dataclass
class Exchange:
    """One request/answer pair the simulator handled."""

    #: The request PIDs the interchange carried, in message order.
    request_pids: tuple[int | None, ...]
    request_valid: bool
    reply: Reply
    #: `False` when some request carried a PID the profiles have no rules for,
    #: so `request_valid` was decided vacuously.
    request_checked: bool = True

    @property
    def request_pid(self) -> int | None:
        """The request PID, for the single-message case."""
        return _only("request_pid", self.request_pids)


class MarktpartnerSim:
    """A counterparty that answers per the AHB, in EDIFACT.

    ::

        nb = MarktpartnerSim(
            mp_id="9900357000003", rolle="NB", reference_date="2026-04-01"
        )
        nb.on(55001).bestaetigung(antwort_code="A51", ebd="E_0623")
        nb.on(55004).ablehnung(antwort_code="A06")
        nb.on(55016).timeout()

        reply = nb.receive(interchange_bytes)
        platform.ingest(reply.business)      # a real interchange, not a dict
    """

    def __init__(
        self,
        *,
        mp_id: str,
        rolle: str,
        reference_date: str,
        strict: bool = True,
    ) -> None:
        """
        `reference_date` is the date the exchange happens. It selects the format
        version for both the incoming validation and the outgoing build, so a
        request and its answer can never be built against different profiles.

        `strict` makes the partner refuse a request that fails validation
        instead of answering it — which is what a conformant partner does. Set
        it `False` and the partner acknowledges and answers anyway, which is how
        a platform is tested against a counterparty that processed a message it
        should have rejected.
        """
        self.mp_id = mp_id
        self.rolle = rolle
        self.reference_date = reference_date
        self.strict = strict
        self._answers: dict[tuple[int, bool], Answer] = {}
        self._sent = 0
        self.exchanges: list[Exchange] = []
        #: What this counterparty has accepted and not yet closed. A repeat
        #: request for a Lokation held here takes the `bei_offenem_vorgang()`
        #: binding.
        self.vorgaenge = Vorgangsregister()

    # ── Configuration ─────────────────────────────────────────────────────────

    def on(self, pid: int) -> Rulebook:
        """Bind an answer to the request Prüfidentifikator `pid`.

        Answers bind to **request** PIDs. 55002 and 55003 are the answers *to*
        55001, not things a partner can be asked, so binding one raises rather
        than registering a rule that can never fire — including through
        `.timeout()` and `.antwort()`, which do not consult the answer table and
        would otherwise accept it silently.
        """
        if _is_answer_pid(pid):
            anfrage = _anfrage_for(pid)
            raise ValueError(
                f"PID {pid} is an answer to {anfrage}, not a request a partner "
                f"can be asked. Bind the request instead: .on({anfrage})."
            )
        return Rulebook(self, pid)

    def _register(self, pid: int, answer: Answer) -> MarktpartnerSim:
        self._answers[(pid, answer.nur_bei_offenem_vorgang)] = answer
        return self

    def _answer_for(self, pid: int | None, vorgang) -> Answer | None:
        """The binding that applies to this request, given what is already open.

        A repeat binding wins when the counterparty holds the Lokation, and
        falls back to the unconditional one when it does not — so binding only
        the repeat case is not a way to accidentally answer nothing.
        """
        if pid is None:
            return None
        if self._occupied_lokation(vorgang) is not None:
            repeat = self._answers.get((pid, True))
            if repeat is not None:
                return repeat
        return self._answers.get((pid, False))

    def _occupied_lokation(self, vorgang) -> str | None:
        """The Lokation of `vorgang` the register already holds open, if any."""
        if vorgang is None:
            return None
        for _, lokation in vorgang.locations:
            if self.vorgaenge.offen(lokation) is not None:
                return lokation
        return None

    @staticmethod
    def _lokation_of(vorgang) -> str | None:
        """The Lokation a Vorgang runs against — the first `SG5 LOC` it names."""
        if vorgang is None:
            return None
        return next((lokation for _, lokation in vorgang.locations), None)

    # ── Behaviour ─────────────────────────────────────────────────────────────

    def receive(self, raw: bytes, *, received_at: str | None = None) -> Reply:
        """Handle one inbound interchange and produce the wire reply.

        `raw` must be a full interchange (`UNB`…`UNZ`): that is the unit a
        market partner receives, and the acknowledgement has to echo its
        Datenaustauschreferenz.

        Every message in the interchange is considered. Those with a binding are
        answered, in request order, inside **one** business-answer interchange —
        which is what a real partner sends and what lets a test assert that the
        second Vorgang was not dropped.

        `received_at` (RFC 3339) is when the request arrived. It is what the
        published answer Frist is measured from — pass it whenever the test is
        about a deadline, and read `Reply.due_at` and `Reply.answered_at`.

        Bytes that are not EDIFACT at all raise rather than producing a reply: a
        CONTRL has to echo the Datenaustauschreferenz of what it rejects, and
        there is none to read. Build that case with `build_contrl` and an
        explicit reference.
        """
        if not raw.lstrip().startswith(b"UNB"):
            raise ValueError(
                "a Marktpartner receives interchanges, not bare messages — wrap "
                "the message with build_interchange() first, or the "
                "acknowledgement has no Datenaustauschreferenz to echo"
            )

        report = validate_edifact(raw, self.reference_date)
        pids = tuple(m.pruefidentifikator for m in report.messages)
        checked = bool(report.messages) and all(m.rules_applied for m in report.messages)
        sender = report.envelope.sender_id if report.envelope else self.mp_id

        # The first Vorgang of each message is what the binding is selected
        # against: a repeat request for an occupied Lokation takes a different
        # answer from the first one.
        vorgaenge = [(m.vorgaenge[0] if m.vorgaenge else None) for m in report.messages]
        bindings = [
            (index, pid, self._answer_for(pid, vorgaenge[index]))
            for index, pid in enumerate(pids)
            if pid is not None
        ]
        # Silence is per-interchange, not per-message: a partner that stayed
        # silent about one Vorgang cannot have acknowledged the envelope that
        # carried it.
        if any(a is not None and a.modus == "timeout" for _, _, a in bindings):
            return self._record(pids, report.is_valid, Reply(), checked)

        # A lax partner processes an invalid request as though it were sound;
        # only a strict one refuses it.
        if not report.is_valid and self.strict:
            return self._record(pids, False, self._refuse(raw, report, sender), checked)

        # A conformant partner acknowledges before it answers.
        ack = self._wrap(
            build_contrl_for(raw, on=self.reference_date, accept=True), to=sender
        )
        answered = [(i, a) for i, _, a in bindings if a is not None]
        if not answered:
            return self._record(
                pids, report.is_valid, Reply(ack=ack, ack_kind="CONTRL"), checked
            )

        messages = [
            build_answer(
                raw,
                answer.pid,
                on=self.reference_date,
                message_ref=str(position + 1),
                antwort_code=answer.antwort_code,
                antwort_ebd=answer.wire_codeliste,
                process_dates=list(answer.process_dates),
                references=list(answer.references),
                free_texts=list(answer.free_texts),
                message_index=index,
            )
            for position, (index, answer) in enumerate(answered)
        ]
        answered_pids = [pid for _, pid, a in bindings if a is not None]
        delay = max(a.delay_werktage for _, a in answered)
        for index, answer in answered:
            self._record_vorgang(vorgaenge[index], pids[index], answer)
        return self._record(
            pids,
            report.is_valid,
            Reply(
                ack=ack,
                ack_kind="CONTRL",
                business=self._wrap_all(messages, to=sender),
                pids=tuple(a.pid for _, a in answered if a.pid is not None),
                modi=tuple(a.modus for _, a in answered),
                antwort_codes=tuple(a.antwort_code for _, a in answered),
                due_ats=tuple(self._due_at(pid, received_at) for pid in answered_pids),
                answered_at=self._answered_at(received_at, delay),
            ),
            checked,
        )

    def _record_vorgang(self, vorgang, pid: int | None, answer: Answer) -> None:
        """Open a Vorgang for an accepted request, so a repeat meets it.

        Only a Bestätigung opens one: an Ablehnung leaves the Lokation free, and
        a counterparty that held it after refusing would refuse the corrected
        resubmission too.
        """
        if answer.modus != "bestaetigung" or pid is None:
            return
        lokation = self._lokation_of(vorgang)
        if lokation is None or self.vorgaenge.offen(lokation) is not None:
            return
        self.vorgaenge.eroeffnen(
            OffenerVorgang(
                lokation=lokation,
                vorgangsnummer=vorgang.vorgangsnummer if vorgang else None,
                pid=pid,
                antwort_pid=answer.pid,
            )
        )

    def _refuse(self, raw: bytes, report, sender: str) -> Reply:
        """Refuse an invalid request with the acknowledgement its failure earns."""
        first = next(iter(report.errors), None)
        broken_envelope = report.envelope is not None and not (
            report.envelope.is_structurally_valid
        )
        syntax = any(f.rule_origin in _SYNTAX_LAYERS for f in report.errors)
        invalid = [m for m in report.messages if not m.is_valid]
        # A broken envelope is a syntax outcome whatever the messages inside say,
        # and it is the case where there may be no invalid message to point an
        # APERAK at: RFF+ACW names a UNH, and an envelope fault has none.
        if syntax or broken_envelope or not invalid:
            return Reply(
                ack=self._wrap(
                    build_contrl_for(raw, on=self.reference_date, accept=False),
                    to=sender,
                ),
                ack_kind="CONTRL",
                ack_positive=False,
            )
        # One APERAK per message: RFF+ACW carries the acknowledged UNH
        # reference, so a single one could only ever name one of them.
        return Reply(
            ack=self._wrap_all(
                [
                    build_aperak_for(
                        raw,
                        on=self.reference_date,
                        error_code="Z10",
                        error_text=_wire_text(first.message if first else None),
                        message_ref=str(index + 1),
                        message_index=index,
                    )
                    for index in [m.index for m in invalid]
                ],
                to=sender,
            ),
            ack_kind="APERAK",
            ack_positive=False,
        )

    def _wrap(self, message: bytes, *, to: str) -> bytes:
        return self._wrap_all([message], to=to)

    def _wrap_all(self, messages: list[bytes], *, to: str) -> bytes:
        return build_interchange(
            sender=self.mp_id,
            receiver=to,
            dar=self._next_dar(),
            messages=messages,
            on=self.reference_date,
        )

    def _next_dar(self) -> str:
        """The next Datenaustauschreferenz for an outbound interchange.

        UNB DE0020 identifies the interchange to the receiver, and a partner
        that reused one would be sending a duplicate every conformant receiver
        is entitled to discard. A counter rather than a clock or a UUID, so a
        scenario stays byte-reproducible.
        """
        self._sent += 1
        return f"MT{self._sent:06d}"

    def _record(
        self,
        pids: tuple[int | None, ...],
        valid: bool,
        reply: Reply,
        checked: bool,
    ) -> Reply:
        self.exchanges.append(Exchange(pids, valid, reply, checked))
        return reply

    def _due_at(self, pid: int | None, received_at: str | None) -> str | None:
        if pid is None or received_at is None:
            return None
        obligation = antwort_obligation(pid)
        return obligation.due_at(received_at) if obligation else None

    def _answered_at(self, received_at: str | None, delay_werktage: int) -> str | None:
        if received_at is None:
            return None
        if delay_werktage <= 0:
            return received_at
        return deadline_at_werktage(received_at, delay_werktage)

    # ── Inspection ────────────────────────────────────────────────────────────

    @property
    def unvalidated_requests(self) -> list[int]:
        """PIDs received whose AHB rules were never applied.

        Non-empty means some assertion in the test passed vacuously: the request
        "validated" because the profile set has no rules for that PID, not
        because it was correct.
        """
        return [
            pid
            for exchange in self.exchanges
            if not exchange.request_checked
            for pid in exchange.request_pids
            if pid is not None
        ]

    def __repr__(self) -> str:
        return (
            f"MarktpartnerSim(mp_id={self.mp_id!r}, rolle={self.rolle!r}, "
            f"reference_date={self.reference_date!r}, "
            f"answers={len(self._answers)}, exchanges={len(self.exchanges)})"
        )


#: `answer PID -> the request it answers`, resolved once from the AHB answer
#: table. Built by walking the table's **request** side, so it can only name
#: pairs the table really has — and asked per answer kind rather than through
#: `answer_pids`, which reports nothing for an asymmetric family like GeLi Gas
#: 44020 (confirmable, never rejectable) and would leave 44021 unrecognised.
_ANSWER_TO_ANFRAGE: dict[int, int] = {}

#: The UTILMD Prüfidentifikator bands: 44xxx Gas, 55xxx Strom.
_UTILMD_PID_RANGE = range(44_000, 56_000)


def _answer_index() -> dict[int, int]:
    if not _ANSWER_TO_ANFRAGE:
        for anfrage in _UTILMD_PID_RANGE:
            for answer in (bestaetigung_pid(anfrage), ablehnung_pid(anfrage)):
                if answer is not None:
                    _ANSWER_TO_ANFRAGE.setdefault(answer, anfrage)
    return _ANSWER_TO_ANFRAGE


def _is_answer_pid(pid: int) -> bool:
    return pid in _answer_index()


def _anfrage_for(pid: int) -> int:
    return _answer_index()[pid]
