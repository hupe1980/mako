"""Type stubs for the compiled Rust core (`makotest._native`).

`py.typed` ships with the wheel, so these stubs are the only thing a consumer's
type checker sees. `tests/test_stubs.py` pins the file against the compiled
module in both directions, so a binding added or removed in Rust without a
matching entry here fails the suite.
"""

from __future__ import annotations

# ── Validation ────────────────────────────────────────────────────────────────

class Finding:
    """One validation finding."""

    severity: str
    """`"critical"`, `"error"`, `"warning"` or `"info"`."""
    rule_id: str | None
    """The rule that fired, e.g. `"SEM-MSCONS-LOCATION-FORMAT"`."""
    rule_origin: str | None
    """`"parse"`, `"directory"`, `"mig"`, `"ahb"`, `"semantic"` or `"custom"`."""
    error_code: str | None
    segment: str | None
    segment_group: str | None
    element: int | None
    component: int | None
    suggestion: str | None
    message: str
    @property
    def position(self) -> str | None:
        """`"LOC[2].0"` — where the finding points, or `None`."""

    @property
    def is_error(self) -> bool: ...

class Vorgang:
    """One SG4 Vorgang as it stands on the wire, read back off a parsed message."""

    vorgangsnummer: str | None
    locations: list[tuple[str, str]]
    dates: list[tuple[str, str]]
    references: list[tuple[str, str]]
    transaktionsgrund: str | None
    antwort_code: str | None
    antwort_codeliste: str | None
    def location(self, lokationstyp: str) -> str | None: ...
    def date(self, qualifier: str) -> str | None: ...
    def iso_date(self, qualifier: str) -> str | None: ...
    def reference(self, qualifier: str) -> str | None: ...

class MessageReport:
    """The validation outcome for one message inside an interchange."""

    index: int
    message_ref: str
    pruefidentifikator: int | None
    message_type: str | None
    release: str | None
    is_valid: bool
    rules_applied: bool
    """`False` when this PID has no AHB rules, so `is_valid` means nothing."""
    findings: list[Finding]
    def by_rule(self, prefix: str) -> list[Finding]: ...
    @property
    def errors(self) -> list[Finding]: ...

class Envelope:
    """The UNB/UNZ envelope, as the receiving platform reads it."""

    sender_id: str
    sender_qualifier: str
    receiver_id: str
    receiver_qualifier: str
    control_ref: str
    transmission_date: str | None
    test_indicator: bool
    message_count: int
    declared_message_count: int
    is_structurally_valid: bool

class ValidationReport:
    """The outcome of parsing and validating one interchange."""

    is_valid: bool
    envelope: Envelope | None
    messages: list[MessageReport]
    @property
    def pruefidentifikator(self) -> int | None:
        """Raises `ValueError` on a multi-message interchange — use `messages`."""

    @property
    def message_type(self) -> str | None: ...
    @property
    def release(self) -> str | None: ...
    @property
    def rules_applied(self) -> bool: ...
    @property
    def findings(self) -> list[Finding]: ...
    @property
    def errors(self) -> list[Finding]: ...
    def by_rule(self, prefix: str) -> list[Finding]: ...

def validate_edifact(raw: bytes, on: str) -> ValidationReport:
    """Parse and validate an interchange (MIG + AHB + semantic) as of `on`."""

# ── Builders ──────────────────────────────────────────────────────────────────

class UtilmdTransaction:
    """One SG4/IDE transaction of a UTILMD message."""

    vorgangsnummer: str
    """`IDE+24` DE 7402 — the sender's own reference for this Vorgang."""
    transaktionsgrund: str | None
    """SG4 `STS+7` DE 9013 element 2, e.g. `"E01"`."""
    transaktionsgrund_ergaenzung: str | None
    """SG4 `STS+7` element 3; defaults to `"ZW4"` when a Grund is set."""
    antwort_code: str | None
    """SG4 `STS+E01` DE 9013 — the EBD Antwortcode on a Bestätigung/Ablehnung."""
    antwort_ebd: str | None
    """SG4 `STS+E01` DE 1131 — the EBD it comes from, e.g. `"E_0624"`."""
    dates: list[tuple[str, str]]
    """`(qualifier, YYYYMMDD)` SG4 DTM pairs, e.g. `("92", "20261101")` Beginn zum.

    `163`/`164` are Messperioden-Qualifier and never occur at SG4 level.
    """
    references: list[tuple[str, str]]
    """`(qualifier, value)`, e.g. `("Z13", "55001")`."""
    locations: list[tuple[str, str]]
    """`(Lokationstyp, id)` SG5 LOC pairs — `("malo", …)`, `("melo", …)`."""
    customers: list[tuple[str, str]]
    free_texts: list[tuple[str, str]]
    antwort_dritter: str | None
    """`SG4 STS+Z35` — the third party's answer code, from `E_0624`."""
    bilanzkreis: str | None
    """The Bilanzkreis, rendered as the whole `SG8 SEQ+Z79` Produktpaket."""
    def __init__(
        self,
        vorgangsnummer: str,
        transaktionsgrund: str | None = None,
        transaktionsgrund_ergaenzung: str | None = None,
        antwort_code: str | None = None,
        antwort_ebd: str | None = None,
        dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
        locations: list[tuple[str, str]] | None = None,
        customers: list[tuple[str, str]] | None = None,
        free_texts: list[tuple[str, str]] | None = None,
        antwort_dritter: str | None = None,
        bilanzkreis: str | None = None,
    ) -> None: ...

def build_utilmd(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    *,
    on: str | None = None,
    release: str | None = None,
    sparte: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
    document_code: str = "E01",
    references: list[tuple[str, str]] | None = None,
    transactions: list[UtilmdTransaction] | None = None,
) -> bytes: ...
def build_mscons(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    metering_point: str,
    quantities: list[tuple[str, str, str]] | None = None,
    *,
    intervals: list[tuple[str, str, str, str, str]] | None = None,
    on: str | None = None,
    release: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
    obis: str | None = None,
) -> bytes:
    """`quantities` are `(qualifier, value, unit)`; `intervals` add `(start, end)`.

    Interval data needs `intervals`: a bare `QTY` carries no time reference.
    """

def build_aperak(
    sender: str,
    receiver: str,
    *,
    on: str | None = None,
    release: str | None = None,
    pruefidentifikator: int | None = None,
    acw_ref: str | None = None,
    error_code: str | None = None,
    error_text: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
    document_code: str | None = None,
) -> bytes: ...
def build_contrl(
    sender: str,
    receiver: str,
    interchange_ref: str,
    *,
    on: str | None = None,
    release: str | None = None,
    accept: bool = True,
    message_ref: str = "1",
    action_code: str | None = None,
    syntax_error: str | None = None,
) -> bytes: ...
def build_aperak_for(
    received: bytes,
    *,
    on: str | None = None,
    release: str | None = None,
    error_code: str | None = None,
    error_text: str | None = None,
    pruefidentifikator: int | None = None,
    message_ref: str = "1",
    message_index: int = 0,
) -> bytes:
    """The APERAK acknowledging one message of `received`, mirror fields derived."""

def build_contrl_for(
    received: bytes,
    *,
    on: str | None = None,
    release: str | None = None,
    accept: bool = True,
    message_ref: str = "1",
) -> bytes:
    """The CONTRL acknowledging the interchange `received`."""

def build_answer(
    received: bytes,
    answer_pid: int,
    *,
    on: str | None = None,
    release: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
    document_code: str | None = None,
    antwort_code: str | None = None,
    antwort_ebd: str | None = None,
    process_dates: list[tuple[str, str]] | None = None,
    references: list[tuple[str, str]] | None = None,
    free_texts: list[tuple[str, str]] | None = None,
    message_index: int = 0,
) -> bytes:
    """The UTILMD business answer to one message of `received`, under `answer_pid`."""

class Positionsfehler:
    """One refused Rechnungsposition of a REMADV."""

    positionsnummer: int
    gruende: list[tuple[str, str]]
    erlaeuterung: str | None
    def __init__(
        self,
        positionsnummer: int,
        gruende: list[tuple[str, str]],
        erlaeuterung: str | None = None,
    ) -> None: ...

def build_iftsta(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    *,
    status: tuple[str, str] | None = None,
    vorgangsnummer: str | None = None,
    order_reference: str | None = None,
    vertragsende: str | None = None,
    on: str | None = None,
    release: str | None = None,
    document_code: str | None = None,
    document_id: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
) -> bytes:
    """The WiM status message — `SG15 STS` is a (category, reason) pair."""

def build_quotes(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    *,
    location: str | None = None,
    bindungsfrist: tuple[str, str] | None = None,
    product: str | None = None,
    price: str | None = None,
    contact: tuple[str, str] | None = None,
    on: str | None = None,
    release: str | None = None,
    document_code: str | None = None,
    document_id: str | None = None,
    references: list[tuple[str, str]] | None = None,
    currency: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
) -> bytes:
    """The ESA Angebot — `bindungsfrist` is a count plus a unit, never a date."""

def build_orders(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    *,
    location: str | None = None,
    abonnement: str | None = None,
    ausfuehrungsdatum: str | None = None,
    on: str | None = None,
    release: str | None = None,
    document_code: str | None = None,
    document_id: str | None = None,
    references: list[tuple[str, str]] | None = None,
    item_description: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
) -> bytes:
    """The WiM / ESA / Sperrung request an ORDRSP answers."""

def build_ordrsp(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    *,
    antwort_code: str | None = None,
    antwort_ebd: str | None = None,
    on: str | None = None,
    release: str | None = None,
    abonnement: str | None = None,
    adjustment_reason: str | None = None,
    document_code: str | None = None,
    document_id: str | None = None,
    references: list[tuple[str, str]] | None = None,
    line_item: bool = False,
    item_description: bool = False,
    message_ref: str = "1",
    document_date: str | None = None,
) -> bytes:
    """The answer to an ORDERS — its `SG2 AJT` carries the Antwortcode and EBD."""

def build_remadv(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    *,
    rechnungsnummer: str,
    faelliger_betrag: str,
    ueberweisungsbetrag: str,
    rechnungsdatum: str,
    dokumentenart: str = "380",
    on: str | None = None,
    release: str | None = None,
    kopf_gruende: list[tuple[str, str]] | None = None,
    positionsfehler: list[Positionsfehler] | None = None,
    waehrung: str | None = None,
    message_ref: str = "1",
    document_id: str | None = None,
    document_date: str | None = None,
) -> bytes:
    """The answer to an invoice: a Zahlungsavis or a Rückmeldung with its AJT."""

def build_interchange(
    sender: str,
    receiver: str,
    dar: str,
    messages: list[bytes],
    *,
    on: str | None = None,
    date: str | None = None,
    time: str = "0000",
) -> bytes: ...

# ── Identifiers ───────────────────────────────────────────────────────────────

def malo_is_valid(value: str) -> bool: ...
def malo_check_digit(base: str) -> int: ...
def malo_from_base(base: str) -> str: ...
def melo_is_valid(value: str) -> bool: ...
def mp_id_is_valid(value: str) -> bool: ...
def mp_id_check_digit_schemes(value: str) -> list[str]:
    """Every procedure `value` satisfies — `"bdew"`, `"gln"`, or both."""

def mp_id_from_base(base: str, scheme: str = "bdew") -> str: ...
def mp_id_authority(value: str) -> str: ...
def mp_id_unb_qualifier(value: str) -> str: ...
def eic_from_prefix(prefix: str) -> str: ...
def eic_is_valid(value: str) -> bool: ...
def eic_type_char(value: str) -> str: ...
def bilanzkreis_from_prefix(prefix: str) -> str:
    """A Bilanzkreis is an EIC **Party** — object type `X`."""

def bilanzkreis_is_valid(value: str) -> bool: ...
def bilanzierungsgebiet_from_prefix(prefix: str) -> str:
    """A Bilanzierungsgebiet is an EIC **Area** — object type `Y`."""

def bilanzierungsgebiet_is_valid(value: str) -> bool: ...
def resource_id_from_base(kind: str, base: str) -> str: ...
def resource_id_is_valid(kind: str, value: str) -> bool: ...
def resource_id_kinds() -> list[tuple[str, str]]: ...

# ── Fristen ───────────────────────────────────────────────────────────────────

def is_werktag(date: str) -> bool: ...
def add_werktage(date: str, n: int) -> str: ...
def next_werktag(date: str) -> str: ...
def deadline_at_werktage(received: str, werktage: int) -> str:
    """The WiM shape: 17:00 Europe/Berlin on the n-th Werktag."""

def end_of_werktag_after(received: str, werktage: int) -> str:
    """The GeLi Gas shape: the end of the n-th Werktag."""

def next_werktag_at(received: str, at: str) -> str:
    """The GPKE shape: `at` on the first Werktag after the Übertragungstag."""

def add_hours(received: str, hours: int) -> str: ...
def contrl_due_at(received: str) -> str: ...
def aperak_strom_due_at(received: str) -> str: ...
def aperak_gas_folgeprozess_due_at(received: str) -> str: ...
def aperak_gas_initialprozess_due_at(received: str) -> str: ...
def format_303(instant: str) -> str:
    """An RFC 3339 instant as EDIFACT DE 2379 format `303` — `CCYYMMDDHHMMZZZ`."""

def berlin_day_bounds(date: str) -> tuple[str, str]:
    """The half-open UTC bounds of one Europe/Berlin day — 23, 24 or 25 hours."""

def berlin_instant(date: str, at: str) -> str:
    """`at` (`"HH:MM[:SS]"`) on `date`, with that date's own Europe/Berlin offset."""

def berlin_mtu_count(date: str, mtu_minutes: int) -> int:
    """Market time units the Europe/Berlin day `date` has — 92, 96 or 100 at 15 min."""

class AntwortObligation:
    """One published answer obligation: who owes what, by when, and where it says so."""

    trigger_pid: int
    name: str
    answered_by: str
    bestaetigung_pid: int
    ablehnung_pid: int
    ebd: str | None
    family: str
    """`"gpke"`, `"geli-gas"`, `"wim"` or `"wim-gas"`."""
    shape: str
    """`"werktag_at"`, `"same_day_at"`, `"same_day"`, `"end_of_werktag"` or
    `"werktage_at_cutoff"`."""
    werktage: int | None
    clock_time: str | None
    source: str
    @property
    def window(self) -> str: ...
    def due_at(self, received: str) -> str: ...

def antwort_obligation(trigger_pid: int) -> AntwortObligation | None: ...
def antwort_obligations() -> list[AntwortObligation]: ...
def antwort_deadline(trigger_pid: int, received: str) -> str | None: ...

# ── Antwortcodes (Entscheidungsbaum Codelisten) ───────────────────────────────

class AntwortCode:
    """One published Antwortcode, resolved against the tree that publishes it."""

    code: str
    tree: str
    wire_codeliste: str | None
    cluster: str
    bedeutung: str
    braucht_bemerkung: bool
    @property
    def ist_zustimmung(self) -> bool | None: ...

def entscheidungsbaeume() -> list[str]: ...
def antwort_code(tree: str, code: str) -> AntwortCode | None: ...
def antwort_codes(tree: str) -> list[AntwortCode]: ...
def antwort_codes_for_pid(trigger_pid: int) -> list[AntwortCode]: ...

# ── Prüfidentifikatoren and releases ──────────────────────────────────────────

def pruefidentifikatoren(
    message_type: str, on: str | None = None, sparte: str | None = None
) -> list[int]: ...
def pid_has_ahb_rules(
    message_type: str, pid: int, on: str | None = None, sparte: str | None = None
) -> bool: ...
def message_types_of(pid: int) -> list[str]:
    """A list: APERAK and COMDIS both declare 29001 and 29002."""

def pid_carrying_message_types() -> list[str]:
    """Every EDIFACT type the BDEW assigns Prüfidentifikatoren to, sorted."""

def answer_pids(anfrage: int) -> tuple[int, int] | None: ...
def bestaetigung_pid(anfrage: int) -> int | None: ...
def ablehnung_pid(anfrage: int) -> int | None: ...
def release_for(message_type: str, on: str, sparte: str | None = None) -> str | None: ...
def format_versions() -> list[str]: ...
def releases(message_type: str) -> list[str]: ...

# ── CloudEvents ───────────────────────────────────────────────────────────────

def event_types() -> list[str]:
    """Every CloudEvents `type` the platform declares, sorted."""

def event_type_exists(event_type: str) -> bool:
    """`True` when `event_type` is declared — a typo or a rename is not."""

def event_matches(pattern: str, event_type: str) -> bool:
    """The platform's own subscription glob: `*` any sequence, `?` one char."""

def event_types_matching(pattern: str) -> list[str]:
    """Every declared type `pattern` would deliver. Empty = a dead subscription."""

def cloudevent_core_attributes() -> list[str]:
    """The nine CloudEvents 1.0 core attribute names."""

def cloudevent_json_members() -> list[str]:
    """The nine core attributes plus `data_base64`, the JSON format's binary carrier."""

def is_valid_extension_key(key: str) -> bool:
    """`True` for a §3.3-legal extension name: lowercase alphanumeric, non-core."""

def parse_cloudevent_time(value: str) -> str:
    """Validate an RFC 3339 `time` attribute, returning it normalised."""

# ── Platform ──────────────────────────────────────────────────────────────────

def bo4e_schema_version() -> str:
    """The BO4E generation the bundled object model is generated from."""
