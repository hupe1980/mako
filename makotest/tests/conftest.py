"""Shared fixtures for makotest's own suite.

The reference date is pinned to a published format version rather than to
"today": every message below is built and validated on it, and a suite whose
meaning changes at the next Formatumstellung is not a suite.
"""

import pytest

from makotest import UtilmdTransaction, build_interchange, build_utilmd
from makotest.plugin import DEFAULT_REFERENCE_DATE, LF_ID, NB_ID

#: FV2026-04-01 — the earliest version on which every message type the toolkit
#: builds is active (CONTRL only enters the compiled set on FV2026-01-01).
ON = DEFAULT_REFERENCE_DATE

#: A 33-character Messlokations-ID: the MSCONS MIG worked example for LOC+172.
MELO = "DE00014559929E00856996N5139699L01"
#: A check-digit-valid Marktlokations-ID.
MALO = "51238696012"


def utilmd_interchange(
    pid: int = 55001,
    *,
    sender: str = LF_ID,
    receiver: str = NB_ID,
    melo: str = MELO,
    document_code: str = "E01",
    on: str = ON,
    dar: str = "REF1",
) -> bytes:
    """A structurally valid UTILMD interchange carrying `pid`."""
    msg = build_utilmd(
        pid,
        sender,
        receiver,
        on=on,
        message_ref="MSG-1",
        document_code=document_code,
        transactions=[
            UtilmdTransaction(
                "melo",
                melo,
                process_dates=[("163", "20260501")],
                references=[("Z13", str(pid))],
            )
        ],
    )
    return build_interchange(
        sender=sender, receiver=receiver, dar=dar, messages=[msg], on=on, time="0915"
    )


@pytest.fixture
def on() -> str:
    return ON


@pytest.fixture
def anmeldung() -> bytes:
    """A valid GPKE Anmeldung (55001) interchange, LF → NB."""
    return utilmd_interchange(55001)
