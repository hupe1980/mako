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

    object_type: str
    """`"malo"`, `"melo"`, `"nelo"`, `"tranche"`, `"tr"` or `"sr"`."""
    object_id: str
    transaktionsgrund: str | None
    process_dates: list[tuple[str, str]]
    """`(qualifier, YYYYMMDD)`, e.g. `("163", "20261101")` for delivery start."""
    references: list[tuple[str, str]]
    """`(qualifier, value)`, e.g. `("Z13", "55001")`."""
    locations: list[tuple[str, str]]
    customers: list[tuple[str, str]]
    free_texts: list[tuple[str, str]]
    def __init__(
        self,
        object_type: str,
        object_id: str,
        transaktionsgrund: str | None = None,
        process_dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
        locations: list[tuple[str, str]] | None = None,
        customers: list[tuple[str, str]] | None = None,
        free_texts: list[tuple[str, str]] | None = None,
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
    quantities: list[tuple[str, str, str]],
    *,
    on: str | None = None,
    release: str | None = None,
    message_ref: str = "1",
    document_date: str | None = None,
    obis: str | None = None,
    bilanzierungsgebiet: str | None = None,
) -> bytes: ...
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
    document_code: str = "E01",
    process_dates: list[tuple[str, str]] | None = None,
    references: list[tuple[str, str]] | None = None,
    message_index: int = 0,
) -> bytes:
    """The UTILMD business answer to one message of `received`, under `answer_pid`."""

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
    """`"next_werktag_at"`, `"end_of_werktag"` or `"werktage_at_cutoff"`."""
    werktage: int | None
    clock_time: str | None
    source: str
    def due_at(self, received: str) -> str: ...

def antwort_obligation(trigger_pid: int) -> AntwortObligation | None: ...
def antwort_obligations() -> list[AntwortObligation]: ...
def antwort_deadline(trigger_pid: int, received: str) -> str | None: ...

# ── Prüfidentifikatoren and releases ──────────────────────────────────────────

def pruefidentifikatoren(
    message_type: str, on: str | None = None, sparte: str | None = None
) -> list[int]: ...
def pid_has_ahb_rules(
    message_type: str, pid: int, on: str | None = None, sparte: str | None = None
) -> bool: ...
def message_types_of(pid: int) -> list[str]:
    """A list: APERAK and COMDIS both declare 29001 and 29002."""

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
