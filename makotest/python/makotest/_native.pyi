"""Type stubs for the compiled Rust core (`makotest._native`)."""

from __future__ import annotations

class Finding:
    """One validation finding from `validate_edifact`."""

    severity: str
    """`"error"`, `"warning"`, `"info"` or `"critical"`."""
    rule_id: str | None
    """The rule that fired, e.g. `"SEM-MSCONS-LOCATION-FORMAT"`."""
    segment: str | None
    message: str

class ValidationReport:
    is_valid: bool
    pruefidentifikator: int | None
    message_type: str | None
    findings: list[Finding]
    def by_rule(self, prefix: str) -> list[Finding]: ...

def malo_is_valid(value: str) -> bool: ...
def melo_is_valid(value: str) -> bool: ...
def malo_check_digit(base: str) -> int: ...
def malo_from_base(base: str) -> str: ...
def is_werktag(date: str) -> bool: ...
def add_werktage(date: str, n: int) -> str: ...
def next_werktag(date: str) -> str: ...
def validate_edifact(
    raw: bytes, reference_date: str | None = None
) -> ValidationReport: ...

class UtilmdTransaction:
    """One SG4/IDE transaction of a UTILMD message."""

    object_type: str
    """`"malo"`, `"melo"`, `"nelo"`, `"tranche"`, `"tr"` or `"sr"`."""
    object_id: str
    process_dates: list[tuple[str, str]]
    """`(qualifier, YYYYMMDD)`, e.g. `("163", "20261101")` for delivery start."""
    references: list[tuple[str, str]]
    """`(qualifier, value)`, e.g. `("Z13", "55001")`."""
    def __init__(
        self,
        object_type: str,
        object_id: str,
        process_dates: list[tuple[str, str]] | None = None,
        references: list[tuple[str, str]] | None = None,
    ) -> None: ...

def build_utilmd(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    release: str = "S2.2",
    message_ref: str = "1",
    document_date: str | None = None,
    document_code: str = "E01",
    transactions: list[UtilmdTransaction] | None = None,
) -> bytes: ...
def build_mscons(
    pruefidentifikator: int,
    sender: str,
    receiver: str,
    metering_point: str,
    quantities: list[tuple[str, str, str]],
    release: str = "2.5",
    message_ref: str = "1",
    document_date: str | None = None,
    obis: str | None = None,
) -> bytes: ...
def build_interchange(
    sender: str,
    receiver: str,
    dar: str,
    messages: list[bytes],
    date: str = "000000",
    time: str = "0000",
) -> bytes: ...
