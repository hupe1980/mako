"""The opposing Marktpartner.

Receives an interchange, acknowledges it, then answers with the AHB response for
that Prüfidentifikator — a Bestätigung, an Ablehnung, or nothing at all.

Four things make it more than a stub.

**It answers in EDIFACT**, built by the same Rust builders the platform uses,
with the parties mirrored and the request's SG4 object and references echoed. A
dictionary answer could not be fed back into the system under test.

**It picks the right acknowledgement.** A CONTRL reports a *syntax* failure and
an APERAK an *application* one — different messages, telling the counterparty to
retry different things. The choice comes from the validation layer that fired.

**It answers every message it was sent.** An interchange routinely carries
several, each a separate Vorgang with its own Prüfidentifikator; answering only
the first and calling that the answer is how a broken second one ships.

**It can misbehave on purpose.** `.timeout()` sends nothing at all, which is the
only way to test the platform's own Fristen. `.antwort(pid=…)` answers with a PID
the AHB does not assign. `delay_werktage=` answers after the Frist has expired.
An unconfigured partner sends no business answer either.

It is a plain object with `receive()`, not a server, so an AS4 transport layers
on top instead of being a dependency of everyone who does not need one.

An Ablehnung's EBD code is reported on the `Reply` but **not** written into the
message: which segment carries it is fixed per process by the AHB, and this
toolkit does not guess AHB structure. Pass the segment through `references=`.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Literal

from .._native import (
    ablehnung_pid,
    answer_pids,
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
    #: EBD outcome code, e.g. `"A06"` (conflicting supply). Reported on the
    #: `Reply`; see the module docstring for why it is not written into the
    #: message.
    erc: str | None = None
    #: `(qualifier, YYYYMMDD)` DTM pairs for the answer transaction, e.g.
    #: `("163", "20261101")` for a confirmed Zuordnungsbeginn.
    process_dates: tuple[tuple[str, str], ...] = ()
    #: Extra `(qualifier, value)` RFF pairs for the answer transaction.
    references: tuple[tuple[str, str], ...] = ()
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
    #: The EBD outcome codes per answered message, aligned with `pids`.
    ercs: tuple[str | None, ...] = ()
    #: When the business answer was due, from the platform's own Frist table.
    #: `None` when no Festlegung quantifies the window, or when `receive()` was
    #: called without `received_at`.
    due_at: str | None = None
    #: When the partner sent the answer, once `received_at` was supplied. Later
    #: than `due_at` when the binding asked for a delay — which is the whole
    #: point of being able to ask.
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
    def erc(self) -> str | None:
        """The EBD outcome code, for the single-message case."""
        return _only("erc", self.ercs)


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

    def __init__(self, sim: MarktpartnerSim, pid: int) -> None:
        self._sim = sim
        self._pid = pid

    def bestaetigung(
        self,
        *,
        process_dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
        delay_werktage: int = 0,
    ) -> MarktpartnerSim:
        """Answer with the AHB Bestätigung for this PID."""
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
                delay_werktage=delay_werktage,
            )
        )

    def ablehnung(
        self,
        *,
        erc: str | None = None,
        references: list[tuple[str, str]] | None = None,
        delay_werktage: int = 0,
    ) -> MarktpartnerSim:
        """Answer with the AHB Ablehnung, optionally naming an EBD code."""
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
                erc=erc,
                references=tuple(references or ()),
                delay_werktage=delay_werktage,
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
        erc: str | None = None,
        process_dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
        delay_werktage: int = 0,
    ) -> MarktpartnerSim:
        """Answer with an explicit PID, bypassing the answer table.

        For adversarial cases — answering with the wrong PID is a thing real
        counterparties do, and a platform should reject it.
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
                erc=erc,
                process_dates=tuple(process_dates or ()),
                references=tuple(references or ()),
                delay_werktage=delay_werktage,
            )
        )

    def _bind(self, answer: Answer) -> MarktpartnerSim:
        return self._sim._register(self._pid, answer)


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
        nb.on(55001).bestaetigung(process_dates=[("163", "20260501")])
        nb.on(55004).ablehnung(erc="A06")
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
        it `False` to test how the platform handles a partner that answers a
        message it should have rejected.
        """
        self.mp_id = mp_id
        self.rolle = rolle
        self.reference_date = reference_date
        self.strict = strict
        self._answers: dict[int, Answer] = {}
        self._sent = 0
        self.exchanges: list[Exchange] = []

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
        self._answers[pid] = answer
        return self

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

        bindings = [
            (index, pid, self._answers.get(pid))
            for index, pid in enumerate(pids)
            if pid is not None
        ]
        # Silence is per-interchange, not per-message: a partner that stayed
        # silent about one Vorgang cannot have acknowledged the envelope that
        # carried it.
        if any(a is not None and a.modus == "timeout" for _, _, a in bindings):
            return self._record(pids, report.is_valid, Reply(), checked)

        if not report.is_valid:
            return self._record(pids, False, self._refuse(raw, report, sender), checked)

        # A conformant partner acknowledges before it answers.
        ack = self._wrap(
            build_contrl_for(raw, on=self.reference_date, accept=True), to=sender
        )
        answered = [(i, a) for i, _, a in bindings if a is not None]
        if not answered:
            return self._record(pids, True, Reply(ack=ack, ack_kind="CONTRL"), checked)

        messages = [
            build_answer(
                raw,
                answer.pid,
                on=self.reference_date,
                message_ref=str(position + 1),
                process_dates=list(answer.process_dates),
                references=list(answer.references),
                message_index=index,
            )
            for position, (index, answer) in enumerate(answered)
        ]
        trigger = next(pid for _, pid, a in bindings if a is not None)
        delay = max(a.delay_werktage for _, a in answered)
        return self._record(
            pids,
            True,
            Reply(
                ack=ack,
                ack_kind="CONTRL",
                business=self._wrap_all(messages, to=sender),
                pids=tuple(a.pid for _, a in answered if a.pid is not None),
                modi=tuple(a.modus for _, a in answered),
                ercs=tuple(a.erc for _, a in answered),
                due_at=self._due_at(trigger, received_at),
                answered_at=self._answered_at(received_at, delay),
            ),
            checked,
        )

    def _refuse(self, raw: bytes, report, sender: str) -> Reply:
        """Refuse an invalid request with the acknowledgement its failure earns."""
        if not self.strict:
            return Reply()
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
                        error_text=(first.message[:70] if first else None),
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
