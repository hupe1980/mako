//! GeLi Gas Stammdatenänderung — master-data change process family (44109–44182).
//!
//! The Gas twin of `mako-gpke::stammdatenaenderung`, but with a **genuine
//! Zustimmung/Ablehnung regime** — unlike Strom (asynchronous quality feedback),
//! GeLi Gas lets the Berechtigter reject a change. What it may answer is the
//! Codeliste its Antwort column names in `SG4 STS+E01` DE 1131
//! ([`ANTWORT_CODELISTEN`]), one per column and not one for the family: the
//! Antwort 44115 draws from `G_0016`, which publishes **E15** Zustimmung ohne
//! Korrekturen alone, while the 44121 draws from `G_0025` — **E15**, **E13**
//! Ablehnung (Bilanzierungsproblem), **E17** Ablehnung wg. Fristüberschreitung
//! and **ZE2** Ablehnung Kapazitätsproblem (EBD 4.3 Kap. 13.10–13.12). A
//! Zustimmung applies the change to `marktd`; on a *bilanzierungsrelevante*
//! change dated other than a Monatserster it goes out as **E17** (GeLiGas AWH
//! V1.2 §4.3.2: bila.rel. changes only zum Monatserster with a one-month lead).
//! Non-bila.rel. changes may take effect unverzüglich.
//!
//! Frist for the Antwort: „unverzüglich, spätestens jedoch bis zum Ablauf des
//! **10. WT** nach Eingang der Änderung" (AWH GeLi Gas § 4.3.2 Nr. 2 / Nr. 4),
//! published as `mako_fristen::antwort::STAMMDATEN_ANTWORT_WERKTAGE_GAS`. Five
//! times the Strom window, and genuinely so — not the LFW24 regime.
//!
//! ## Scope
//!
//! The **change families G1–G7** (NB↔LF, NB↔MSB, MSB↔LF, MSB↔wMSB — one Antwort
//! PID per direction, outcome carried as the SG4 STS code) are modeled in full:
//! inbound change → apply MaLo attributes → Zustimmung/Ablehnung. The **Anfrage
//! families G8–G10** — „Anfrage zur Stammdatenänderung": a Berechtigter holds a
//! new value for a Stammdatum and asks the Verantwortlicher to change it (AWH
//! GeLi Gas V1.2 Kap. 4.4.1, S. 85) — open the Verantwortlicher's process with
//! [`GasStammdatenCommand::ReceiveAnfrage`]. It records which groups the Anfrage
//! filled and registers the Antwort window; nothing goes out until the
//! Verantwortlicher has checked it („Nach Prüfung durch den verantwortlichen NB
//! wird das fachliche Ergebnis der Anfrage in die Antwort … übernommen", Kap.
//! 4.4.2 Nr. 2, S. 87) and sends [`GasStammdatenCommand::SendAnfrageAntwort`]
//! with the mapped Antwort PID ([`ANFRAGE_ANTWORT_PAIRS`]). On `ZG2` that answer
//! carries „die zum Änderungsdatum gültigen Stammdaten" (EBD 4.3 Codeliste
//! `G_0031`, S. 742), which makod reads from marktd at its boundary; a
//! structurally invalid Anfrage is refused with an APERAK. WiM Gas
//! Verpflichtungsanfrage (44168–44170) and „Ende MSB" (44183) belong to other
//! workflows and are excluded.
//!
//! # Regulatory basis
//!
//! - **GeLiGas AWH V1.2 (26.03.2026)** § 4.3 (Stammdatenänderung), § 4.3.2 Nr. 2/4 (Frist)
//! - **UTILMD AHB Gas 1.1** — object → PID map
//! - **EBD 4.3** Kap. 13.10–13.12 (E_3010–E_3012, E_3021, E_3023 and their
//!   Codelisten `G_0016`–`G_0030`)
//! - **UTILMD AHB Gas 1.2** Kap. 5.10–5.11 — the Änderung and Antwort columns
//! - **APERAK AHB 1.1** — Gas Folgeprozess APERAK (nächster Werktag 12:00)

use mako_engine::payload::{Stammdaten, UtilmdPayload};
use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    envelope::{Richtung, Verlauf},
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{HolidayCalendar, deadline_at_werktage};

// ── PID model ─────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "geli-gas-stammdatenaenderung";

/// Deadline label for the 10-Werktage Antwort window.
///
/// „Es ist hierfür ein Konzept anzuwenden, dass … die Änderung, Bearbeitung und
/// Übermittlung von geänderten Stammdaten in vorzugebenden Fristen, die
/// insgesamt **10 Werktage** nicht überschreiten, … sicherstellt" — GeLi Gas
/// 3.0 Kap. 4.3. Five times the Strom window
/// ([`mako_fristen::antwort::STAMMDATEN_RUECKMELDUNG_WERKTAGE`]) and genuinely
/// so: Gas gives the Berechtigter a real Zustimmung/Ablehnung, where Strom has
/// only asynchronous quality feedback.
pub const ANTWORT_WINDOW_LABEL: &str = "geli-gas-stammdaten-antwort-window";

/// Change-family `(Änderung PID, Antwort PID, bilanzierungsrelevant)` rows
/// (G1–G7). The Antwort PID is shared across sender directions; the E15/E13/E17
/// outcome is carried in the message, not in the PID.
pub const STAMMDATEN_PAIRS: &[(u32, u32, bool)] = &[
    // G1 — nicht-bilanzierungsrelevante Änderung vom NB
    (44112, 44115, false), // NB → LF
    (44113, 44115, false), // NB → MSB
    // G2 — bilanzierungsrelevante Änderung vom NB (Monatserster)
    (44123, 44124, true), // NB → LF
    // G3 — Änderung der Marktlokationsstruktur vom NB
    (44175, 44176, false), // NB → LF
    // G4 — nicht-bilanzierungsrelevante Änderung vom LF
    (44109, 44111, false), // LF → NB
    // G5 — bilanzierungsrelevante Änderung vom LF (Monatserster)
    (44120, 44121, true), // LF → NB
    // G6 — Änderung vom MSB (mit Abhängigkeiten)
    (44116, 44119, false), // MSB → NB
    (44117, 44119, false), // MSB → LF
    // G7 — Änderung vom MSB (ohne Abhängigkeiten)
    (44159, 44161, false), // MSB → NB
    (44160, 44161, false), // NB → LF (weitergeleitet)
];

/// G8–G10 Anfrage-family PIDs — the full set routed to this workflow (requests
/// plus their Antwort/Ablehnung PIDs). The request PIDs we answer as data owner
/// are in [`ANFRAGE_ANTWORT_PAIRS`]; the remaining Antwort/Ablehnung PIDs are the
/// requester-side responses (initiating an Anfrage ourselves is a follow-up).
/// Excludes 44168–44170 (WiM Gas Verpflichtungsanfrage) and 44183 („Ende MSB").
pub const ANFRAGE_PIDS: &[u32] = &[
    // G8 — Anfrage zur Stammdatenänderung an NB (+ Antworten/Ablehnungen)
    44139, 44140, 44142, 44156, 44157, 44180, 44181, 44182, // G9 — Anfrage an LF
    44150, 44151, 44152, 44137, 44138, // G10 — Anfrage an MSB
    44162, 44163, 44164, 44143, 44145, 44146, 44165, 44167, 44147, 44149, 44166, 44148,
];

/// Anfrage round-trip rows `(Anfrage PID, Antwort PID)` — the Verantwortlicher
/// answers an inbound Anfrage zur Stammdatenänderung with the Antwort PID
/// UTILMD AHB Gas 1.2 Kap. 5.12 („Stammdatenanfrage vom Berechtigten aus
/// gestartet") names for it.
///
/// The Antwort carries the checked result in `SG4 STS+E01`, and on „Gültiges
/// Ergebnis" the Stammdaten valid at the Änderungsdatum. Where a family has a
/// separate Ablehnung PID it is named in the comment; only the Antwort PID is
/// listed.
pub const ANFRAGE_ANTWORT_PAIRS: &[(u32, u32)] = &[
    // Kap. 5.12.1 — nicht bila.rel. Anfrage an NB
    (44139, 44142), // LF → NB
    (44140, 44142), // MSB → NB
    // Kap. 5.12.4 — nicht bila.rel. Anfrage an LF
    (44137, 44138), // NB → LF
    // Kap. 5.12.5 — bila.rel. Anfrage an LF
    (44150, 44151), // NB → LF (Ablehnung 44152)
    // Kap. 5.12.2 — bila.rel. Anfrage an NB mit Abhängigkeiten
    (44156, 44157), // LF → NB
    // Kap. 5.12.3 — Anfrage der Marktlokationsstruktur beim NB
    (44180, 44181), // LF → NB (Ablehnung 44182)
    // Kap. 5.12.7 — Anfrage an MSB ohne Abhängigkeiten. The NB answers the LF's
    // 44162 as Verteiler, after the MSB has answered its 44165 (S. 206).
    (44162, 44163), // LF → NB (Ablehnung 44164)
    (44165, 44167), // NB [Verteiler] → MSB
    (44166, 44167), // NB [Berechtigt] → MSB
];

/// One Antwort column of UTILMD AHB Gas 1.2 Kap. 5.12 — the PID an Anfrage is
/// answered with on a code, and the Codeliste its `SG4 STS+E01` DE 1131 names.
///
/// A family whose Ablehnung is its own PID has two rows: the code decides the
/// PID, because each column admits only its own list (`ZJ5` rides the 44182
/// under `G_0036`, never the 44181).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnfrageAntwort {
    /// The Anfrage PID answered.
    pub anfrage_pid: u32,
    /// The PID the answer goes out as.
    pub antwort_pid: u32,
    /// `SG4 STS+E01` DE 1131 — the codes it admits are the EBD's, read with
    /// `mako_pruefung::codes::lies`.
    pub codeliste: &'static str,
    /// `true` when the column lists `SG4 DTM+92`/`DTM+157` — the Ablehnungen
    /// 44152 and 44182 carry neither.
    pub sg4_daten: bool,
}

/// Every Antwort column mako assembles, read off UTILMD AHB Gas 1.2 Kap.
/// 5.12 (`SG4 STS 1131`) and EBD 4.3 Kap. 13.13–13.15 (the Codelisten):
///
/// | Anfrage | Antwort | Codeliste | Codes | AHB |
/// |---|---|---|---|---|
/// | 44139 LF → NB | 44142 | `G_0031` (`E_3013`) | `ZG0`, `ZG2` | 5.12.1, S. 148 `[4]` |
/// | 44140 MSB → NB | 44142 | `G_0069` (`E_3026`) | `ZG0`, `ZG2` | 5.12.1, S. 148 `[241]` |
/// | 44156 LF → NB | 44157 | `G_0034` (`E_3013`) | `ZG2`, `ZG4` | 5.12.2, S. 163 |
/// | 44180 LF → NB | 44181 | `G_0035` (`E_3013`) | `ZG2` | 5.12.3, S. 168 |
/// | 44180 LF → NB | 44182 | `G_0036` (`E_3013`) | `ZJ5` | 5.12.3, S. 168 |
/// | 44137 NB → LF | 44138 | `G_0040` (`E_3028`) | `ZG0`, `ZG2` | 5.12.4, S. 171 |
/// | 44150 NB → LF | 44151 | `G_0038` (`E_3028`) | `ZG2`, `ZG4` | 5.12.5, S. 176 |
/// | 44150 NB → LF | 44152 | `G_0039` (`E_3028`) | `ZG0` | 5.12.5, S. 176 |
///
/// The Kap. 5.12.7 answers are not here: the NB's 44163/44164 as Verteiler
/// follow the MSB's 44167 to the 44165 it forwards, and the 44167 is the
/// MSB's.
pub const ANFRAGE_ANTWORTEN: &[AnfrageAntwort] = &[
    AnfrageAntwort {
        anfrage_pid: 44139,
        antwort_pid: 44142,
        codeliste: "G_0031",
        sg4_daten: true,
    },
    AnfrageAntwort {
        anfrage_pid: 44140,
        antwort_pid: 44142,
        codeliste: "G_0069",
        sg4_daten: true,
    },
    AnfrageAntwort {
        anfrage_pid: 44156,
        antwort_pid: 44157,
        codeliste: "G_0034",
        sg4_daten: true,
    },
    AnfrageAntwort {
        anfrage_pid: 44180,
        antwort_pid: 44181,
        codeliste: "G_0035",
        sg4_daten: true,
    },
    AnfrageAntwort {
        anfrage_pid: 44180,
        antwort_pid: 44182,
        codeliste: "G_0036",
        sg4_daten: false,
    },
    AnfrageAntwort {
        anfrage_pid: 44137,
        antwort_pid: 44138,
        codeliste: "G_0040",
        sg4_daten: true,
    },
    AnfrageAntwort {
        anfrage_pid: 44150,
        antwort_pid: 44151,
        codeliste: "G_0038",
        sg4_daten: true,
    },
    AnfrageAntwort {
        anfrage_pid: 44150,
        antwort_pid: 44152,
        codeliste: "G_0039",
        sg4_daten: false,
    },
];

/// The Antwort column an Anfrage is answered with on `code`, with the code as
/// its Codeliste publishes it — `None` when no column mako assembles admits
/// the code.
#[must_use]
pub fn anfrage_antwort(
    anfrage_pid: u32,
    code: &str,
) -> Option<(
    &'static AnfrageAntwort,
    &'static mako_pruefung::codes::AntwortCode,
)> {
    anfrage_antworten(anfrage_pid)
        .find_map(|r| mako_pruefung::codes::lies(r.codeliste, code).map(|c| (r, c)))
}

/// The Antwort columns of one Anfrage — empty when mako assembles none.
pub fn anfrage_antworten(anfrage_pid: u32) -> impl Iterator<Item = &'static AnfrageAntwort> {
    ANFRAGE_ANTWORTEN
        .iter()
        .filter(move |r| r.anfrage_pid == anfrage_pid)
}

/// The codes on which an Antwort auf Anfrage carries Stammdaten: `ZG2` „sendet
/// der Verantwortliche bzw. der Verteiler die zum Änderungsdatum gültigen
/// Stammdaten" and `ZG4` „Verantwortlicher übermittelt die zum angefragten
/// Datum gültigen Stammdaten" (EBD 4.3 Kap. 13.13.1, S. 742–743). Every other
/// code is a refusal and carries no data group.
pub const DATEN_CODES: &[&str] = &["ZG2", "ZG4"];

/// One master-data group an Anfrage zur Stammdatenänderung filled.
///
/// The Antwort 44142 carries a group only „Wenn in Änderungsmeldung gefüllt"
/// (Bedingung `[336]`) and „Wenn SG4 STS+E01+ZG2 … vorhanden" (`[202]`) — UTILMD
/// AHB Gas 1.2 Kap. 5.12.1, every data group of the column. So the Anfrage
/// decides what the answer contains, and the process keeps the list. The
/// `SG12` Kunde des NB, Netzanschlusseigentümer and Hausverwalter are `Kann`
/// „freiwillig" (`[558]`) on the 44142 and are not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AngefragteGruppe {
    /// `SG6 RFF+Z18` `DTM+Z21` Termin der Netznutzungsabrechnung, with the
    /// `DTM+Z09` Nächste Netznutzungsabrechnung it makes Muss (`[24]`).
    TerminNetznutzungsabrechnung,
    /// `SG6 RFF+Z18` `DTM+Z22` Netznutzungsabrechnungsintervall.
    Netznutzungsabrechnungsintervall,
    /// `SG6 RFF+Z19` `DTM+752` Geplante Turnusablesung des NB.
    Turnusablesung,
    /// `SG8 SEQ+Z01` `SG10 CCI+Z21` Marktgebiet.
    Marktgebiet,
    /// `SG8 SEQ+Z01` `SG10 CCI+++Y01` Druckebene der Marktlokation.
    Druckebene,
    /// `SG8 SEQ+Z01` `SG10 CCI+++Z88` Netznutzung.
    Netznutzung,
    /// `SG8 SEQ+Z02` OBIS-Daten der Marktlokation.
    ObisDaten,
    /// `SG8 SEQ+Z07` Konzessionsabgabedaten.
    Konzessionsabgabe,
    /// `SG8 SEQ+Z12` Gemeinderabatt.
    Gemeinderabatt,
    /// `SG8 SEQ+Z18` `SG10 CCI+++ZB3` Zugeordnete Marktpartner.
    ZugeordneteMarktpartner,
    /// `SG8 SEQ+Z18` `SG10 CCI+++Y02` Gasqualität.
    Gasqualitaet,
    /// `SG12 NAD+DP` Marktlokationsanschrift.
    Marktlokationsanschrift,
    /// `SG6 RFF+Z18` `DTM+Z20` Abrechnungsintervall des LF (44137 → 44138).
    AbrechnungsintervallLf,
    /// `SG8 SEQ+Z01` `SG10 CCI+++Z15`/`Z18` Gruppenzuordnung nach EnWG (44137
    /// → 44138).
    Gruppenzuordnung,
    /// `SG12 NAD+Z09` Kunde des Lieferanten (44137 → 44138).
    KundeDesLf,
    /// `SG12 NAD+Z04` Korrespondenzanschrift des Kunden des Lieferanten
    /// (44137 → 44138).
    KorrespondenzanschriftKundeLf,
    /// `SG8 SEQ+Z01` `SG10 CCI+Z19` Bilanzkreis (44150 → 44151).
    Bilanzkreis,
}

/// Return the Antwort PID for an inbound Anfrage PID.
#[must_use]
pub fn antwort_for_anfrage(anfrage_pid: u32) -> Option<u32> {
    ANFRAGE_ANTWORT_PAIRS
        .iter()
        .find(|(a, _)| *a == anfrage_pid)
        .map(|(_, r)| *r)
}

/// `true` when `pid` is an Anfrage zur Stammdatenänderung this deployment answers.
#[must_use]
pub fn is_anfrage_request_pid(pid: u32) -> bool {
    ANFRAGE_ANTWORT_PAIRS.iter().any(|(a, _)| *a == pid)
}

/// `true` when `pid` is the **Antwort** to an Anfrage zur Stammdatenänderung —
/// the answer a Berechtigter receives, not a change it must apply.
///
/// mako implements only the answering side of the G8–G10 Anfrage round-trip
/// (there is no `SendAnfrage` command), so no counterparty should send one of
/// these unsolicited. It is separated out anyway because the alternative is
/// worse than dropping it: these PIDs are neither `is_antwort_pid` nor
/// `is_anfrage_request_pid`, so without this predicate they fall through to the
/// Änderung branch and an Antwort would be **applied as a master-data
/// change**.
#[must_use]
pub fn is_anfrage_response_pid(pid: u32) -> bool {
    ANFRAGE_ANTWORT_PAIRS.iter().any(|(_, r)| *r == pid)
        || ANFRAGE_ANTWORTEN.iter().any(|r| r.antwort_pid == pid)
}

/// Return `(Antwort PID, bilanzierungsrelevant)` for an inbound Änderung PID.
#[must_use]
pub fn antwort_for(aenderung_pid: u32) -> Option<(u32, bool)> {
    STAMMDATEN_PAIRS
        .iter()
        .find(|(a, _, _)| *a == aenderung_pid)
        .map(|(_, r, bila)| (*r, *bila))
}

/// `true` when `pid` is an Änderung PID (inbound change → apply + Antwort).
#[must_use]
pub fn is_aenderung_pid(pid: u32) -> bool {
    STAMMDATEN_PAIRS.iter().any(|(a, _, _)| *a == pid)
}

/// `true` when `pid` is an Antwort PID (resumes a change we initiated).
#[must_use]
pub fn is_antwort_pid(pid: u32) -> bool {
    STAMMDATEN_PAIRS.iter().any(|(_, r, _)| *r == pid)
}

/// `true` when `datum` (`YYYYMMDD`) is a Monatserster.
#[must_use]
pub fn is_monatserster(datum: &str) -> bool {
    datum.len() == 8 && datum.ends_with("01")
}

// ── Antwort outcome ─────────────────────────────────────────────────────────────

/// The Codeliste each Antwort auf Änderung draws its `SG4 STS+E01` code from,
/// keyed by the **Änderung** PID it answers — one alphabet per AHB column, not
/// one for the family.
///
/// Read off DE 1131 of each Antwort column (UTILMD AHB Gas 1.2 Kap. 5.10.1 and
/// 5.11.1–5.11.6); the lists themselves are EBD 4.3 Kap. 13.10–13.12. The
/// columns differ: `G_0016` publishes `E15` alone, so a 44115 cannot refuse,
/// while `G_0025` admits `E13`, `E17` and `ZE2`. The 44119 and 44161 columns
/// name two lists, picked by the sender's role — `G_0026` „Wenn MP-ID in SG2
/// NAD+MS … in der Rolle NB" (`[98]`), `G_0027`/`G_0030` „… in der Rolle LF"
/// (`[5]`) — and the Änderung PID says which: the NB answers the MSB's
/// 44116/44159, the LF the NB's forwarded 44117/44160.
pub const ANTWORT_CODELISTEN: &[(u32, &str)] = {
    use mako_pruefung::codes as c;
    &[
        (44112, c::CODELISTE_ANTWORT_AENDERUNG_NB_GAS), // → 44115
        (44113, c::CODELISTE_ANTWORT_AENDERUNG_NB_GAS), // → 44115
        (44123, c::CODELISTE_ANTWORT_BILA_AENDERUNG_NB_GAS), // → 44124
        (44175, c::CODELISTE_ANTWORT_MALO_STRUKTUR_GAS), // → 44176
        (44109, c::CODELISTE_ANTWORT_AENDERUNG_LF_GAS), // → 44111
        (44120, c::CODELISTE_ANTWORT_BILA_AENDERUNG_LF_GAS), // → 44121
        (44116, c::CODELISTE_ANTWORT_AENDERUNG_MSB_NB_GAS), // → 44119
        (44117, c::CODELISTE_ANTWORT_AENDERUNG_MSB_LF_GAS), // → 44119
        (44159, c::CODELISTE_ANTWORT_AENDERUNG_MSB_NB_GAS), // → 44161
        (44160, c::CODELISTE_ANTWORT_AENDERUNG_MSB_OHNE_LF_GAS), // → 44161
    ]
};

/// The Codeliste the Antwort to `aenderung_pid` draws its code from.
#[must_use]
pub fn antwort_codeliste(aenderung_pid: u32) -> Option<&'static str> {
    ANTWORT_CODELISTEN
        .iter()
        .find(|(a, _)| *a == aenderung_pid)
        .map(|(_, cl)| *cl)
}

/// `BGM` DE 1001 `E03` „Änderungsmeldungen" — what an Änderung mako sends
/// is. Every Änderung column lists it (UTILMD AHB Gas 1.2 Kap. 5.10–5.11);
/// `Z26` „Vorläufige Meldung zur Marktraumumstellung" is a Meldung mako does
/// not initiate.
pub const AENDERUNGSMELDUNGEN: &str = "E03";

/// `E15` „Zustimmung ohne Korrekturen" — published by every Antwort-Codeliste
/// of [`ANTWORT_CODELISTEN`] (EBD 4.3 Kap. 13.10–13.12).
pub const ZUSTIMMUNG: &str = "E15";

/// `E17` „Ablehnung wg. Fristüberschreitung" — „Bei der Übermittlung von
/// bilanzierungsrelevanten Stammdatenänderungen wird auch eine Ablehnung
/// erfolgen, wenn das Änderungsdatum kein Monatserster ist." (EBD 4.3 Kap.
/// 13.10.1.2 `G_0019` and 13.11.1.2 `G_0025`, the two bilanzierungsrelevante
/// Antworten).
pub const ABLEHNUNG_FRIST: &str = "E17";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas Stammdatenänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasStammdatenEvent {
    /// Inbound Änderung received (we are the Berechtigter).
    AenderungErhalten {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// GLN of the Verantwortlicher.
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Änderung PID.
        pruefidentifikator: Pruefidentifikator,
        /// `true` for bilanzierungsrelevante changes (Monatserster required).
        bilanzierungsrelevant: bool,
        /// Änderungsdatum (`YYYYMMDD`).
        aenderungsdatum: String,
        /// The MaLo attribute patch to apply **iff** the Antwort is Zustimmung.
        patch: serde_json::Value,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `SG4 DTM+92` Datum Vertragsbeginn (`YYYYMMDD`) — Muss on every
        /// Änderung column (UTILMD AHB Gas 1.2 Kap. 5.10–5.11, Nr 00014).
        #[serde(default)]
        vertragsbeginn: Option<String>,
        /// The Änderung's `IDE+24` — the Antwort's `SG6 RFF+TN`.
        #[serde(default)]
        vorgangsnummer: Option<String>,
        /// The Änderung's `BGM` DE 1001 — `E03` Änderungsmeldungen or `Z26`
        /// Vorläufige Meldung zur Marktraumumstellung.
        #[serde(default)]
        dokumentenart: Option<String>,
    },
    /// Structural validation passed.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Our Antwort dispatched (Zustimmung or Ablehnung).
    AntwortGesendet {
        /// Antwort PID.
        response_pid: Pruefidentifikator,
        /// `SG4 STS+E01` DE 9013.
        #[serde(default)]
        antwort_code: String,
        /// `SG4 STS+E01` DE 1131 — the column's Codeliste
        /// ([`ANTWORT_CODELISTEN`]).
        #[serde(default)]
        antwort_codeliste: String,
    },
    /// Verantwortlicher: we dispatched an Änderung (e.g. LF → NB 44109).
    AenderungGesendet {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// GLN of the Verantwortlicher (us).
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Änderung PID.
        pruefidentifikator: Pruefidentifikator,
        /// Bila-relevance.
        bilanzierungsrelevant: bool,
        /// Änderungsdatum.
        aenderungsdatum: String,
        /// `SG4 DTM+92` Datum Vertragsbeginn.
        #[serde(default)]
        vertragsbeginn: Option<String>,
    },
    /// Initiator: the Berechtigter's Antwort arrived.
    AntwortErhalten {
        /// Antwort PID.
        response_pid: Pruefidentifikator,
        /// `SG4 STS+E01` DE 9013 as it arrived.
        #[serde(default)]
        antwort_code: String,
        /// `SG4 STS+E01` DE 1131 as it arrived.
        #[serde(default)]
        antwort_codeliste: Option<String>,
    },
    /// Verantwortlicher: an Anfrage zur Stammdatenänderung arrived and awaits
    /// its check.
    AnfrageErhalten {
        /// What the answer needs from the Anfrage.
        anfrage: GasAnfrageData,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Verantwortlicher: the Anfrage was answered.
    AnfrageBeantwortet {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Anfrage PID.
        anfrage_pid: Pruefidentifikator,
        /// Antwort PID.
        antwort_pid: Pruefidentifikator,
        /// `SG4 STS+E01` DE 9013 — `ZG2` Gültiges Ergebnis, `ZG0` Ablehnung.
        antwort_code: String,
        /// `SG4 STS+E01` DE 1131 — `G_0031` or `G_0069`.
        antwort_codeliste: String,
    },
    /// Structural validation failed — APERAK dispatched.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired without an Antwort.
    DeadlineExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for GasStammdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AenderungErhalten { .. } => "GasStammdatenAenderungErhalten",
            Self::ValidationPassed { .. } => "GasStammdatenValidationPassed",
            Self::AntwortGesendet { .. } => "GasStammdatenAntwortGesendet",
            Self::AenderungGesendet { .. } => "GasStammdatenAenderungGesendet",
            Self::AntwortErhalten { .. } => "GasStammdatenAntwortErhalten",
            Self::AnfrageErhalten { .. } => "GasStammdatenAnfrageErhalten",
            Self::AnfrageBeantwortet { .. } => "GasStammdatenAnfrageBeantwortet",
            Self::Rejected { .. } => "GasStammdatenRejected",
            Self::DeadlineExpired { .. } => "GasStammdatenDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AenderungErhalten {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet { response_pid, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(response_pid.as_u32()),
                message_ref: None,
            }),
            Self::AenderungGesendet {
                pruefidentifikator, ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: None,
            }),
            Self::AntwortErhalten { response_pid, .. } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(response_pid.as_u32()),
                message_ref: None,
            }),
            Self::AnfrageErhalten {
                anfrage,
                message_ref,
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(anfrage.anfrage_pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            // `handle` queues the Antwort UTILMD with this event; the message it
            // states is that outbound Antwort.
            Self::AnfrageBeantwortet { antwort_pid, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(antwort_pid.as_u32()),
                message_ref: None,
            }),
            Self::ValidationPassed { .. } | Self::DeadlineExpired { .. } => None,
            // Emitted only beside the APERAK 313 Verarbeitbarkeitsfehlermeldung
            // `handle` queues for it; an APERAK carries no Prüfidentifikator.
            Self::Rejected { .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            }),
        }
    }

    /// The code an Antwort carries in `SG4 STS+E01`, with its Codeliste — the
    /// Antwort auf Anfrage and the Antwort auf Änderung, sent or received.
    fn antwort(&self) -> Option<mako_engine::envelope::Antwortcode> {
        match self {
            Self::AnfrageBeantwortet {
                antwort_code,
                antwort_codeliste,
                ..
            }
            | Self::AntwortGesendet {
                antwort_code,
                antwort_codeliste,
                ..
            } => Some(mako_engine::envelope::Antwortcode::new(
                antwort_code.as_str(),
                Some(antwort_codeliste.as_str()),
            )),
            Self::AntwortErhalten {
                antwort_code,
                antwort_codeliste,
                ..
            } => Some(mako_engine::envelope::Antwortcode::new(
                antwort_code.as_str(),
                antwort_codeliste.as_deref(),
            )),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured when the process starts (either role).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GasStammdatenData {
    /// Gas-Marktlokation.
    pub location_id: MaLo,
    /// GLN of the Verantwortlicher.
    pub sender: MarktpartnerCode,
    /// GLN of the Berechtigter.
    pub receiver: MarktpartnerCode,
    /// Änderung PID.
    pub pruefidentifikator: Pruefidentifikator,
    /// Bila-relevance.
    pub bilanzierungsrelevant: bool,
    /// Änderungsdatum (`YYYYMMDD`).
    pub aenderungsdatum: String,
    /// The MaLo attribute patch, applied on Zustimmung. Empty for the
    /// initiator side and for non-MaLo objects.
    #[serde(default)]
    pub patch: serde_json::Value,
    /// `SG4 DTM+92` Datum Vertragsbeginn (`YYYYMMDD`) the Änderung carried —
    /// what the 44176 restates beside the Änderungsdatum.
    #[serde(default)]
    pub vertragsbeginn: Option<String>,
    /// The Änderung's `IDE+24`, which its Antwort names in `SG6 RFF+TN`.
    #[serde(default)]
    pub vorgangsnummer: Option<String>,
    /// The Änderung's `BGM` DE 1001, which its Antwort restates: the 44115
    /// column admits `E03` and `Z26` alike (UTILMD AHB Gas 1.2 Kap. 5.10.1),
    /// and only the Änderung says which Meldung is answered.
    #[serde(default)]
    pub dokumentenart: Option<String>,
}

/// What an Anfrage zur Stammdatenänderung leaves for its Antwort.
///
/// Every value the Antwort restates comes from here: both `SG4 DTM` „Es ist
/// das Datum/ Daten aus der Anfrage zu verwenden" (Bedingung `[528]`), the
/// `SG4 STS+7` Transaktionsgrund, the `SG6 RFF+TN` „Referenz Vorgangsnummer
/// (aus Anfragenachricht)", and which groups it filled (`[336]`) — UTILMD AHB
/// Gas 1.2 Kap. 5.12.1.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GasAnfrageData {
    /// The Meldepunkt (`SG5 LOC+172`) the Anfrage names.
    pub location_id: MaLo,
    /// MP-ID of the Berechtigter that asked.
    pub requester: MarktpartnerCode,
    /// MP-ID of the Verantwortlicher (us).
    pub owner: MarktpartnerCode,
    /// The Anfrage PID.
    pub anfrage_pid: Pruefidentifikator,
    /// The Antwort PID ([`antwort_for_anfrage`]).
    pub antwort_pid: Pruefidentifikator,
    /// `SG4 DTM+157` Gültigkeit, Beginndatum — the Änderungsdatum (`YYYYMMDD`).
    pub aenderungsdatum: String,
    /// `SG4 DTM+92` Datum Vertragsbeginn (`YYYYMMDD`).
    pub vertragsbeginn: Option<String>,
    /// `SG4 STS+7` Transaktionsgrund.
    pub transaktionsgrund: Option<String>,
    /// The Anfrage's `IDE+24` Vorgangsnummer.
    pub vorgangsnummer: Option<String>,
    /// The groups the Anfrage filled.
    pub angefragt: Vec<AngefragteGruppe>,
}

/// State of a GeLi Gas Stammdatenänderung process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GasStammdatenState {
    /// No events yet.
    #[default]
    New,
    /// Berechtigter: inbound Änderung received.
    Eingegangen(GasStammdatenData),
    /// Berechtigter: validation passed; Antwort not yet sent.
    ValidationPassed(GasStammdatenData),
    /// Berechtigter: Antwort dispatched (terminal).
    Beantwortet {
        /// Data from the Änderung.
        data: GasStammdatenData,
        /// `SG4 STS+E01` DE 9013 of the Antwort.
        antwort_code: String,
    },
    /// Verantwortlicher: an Anfrage zur Stammdatenänderung awaits its Antwort.
    AnfrageOffen(GasAnfrageData),
    /// Verantwortlicher: the Anfrage was answered (terminal).
    AnfrageBeantwortet {
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Anfrage PID.
        anfrage_pid: Pruefidentifikator,
        /// Antwort PID.
        antwort_pid: Pruefidentifikator,
        /// `SG4 STS+E01` DE 9013.
        antwort_code: String,
    },
    /// Verantwortlicher: Änderung dispatched; awaiting Antwort.
    Gesendet(GasStammdatenData),
    /// Verantwortlicher: closed by the Berechtigter's Antwort.
    Abgeschlossen {
        /// Data from the Änderung.
        data: GasStammdatenData,
        /// `SG4 STS+E01` DE 9013 of the Berechtigter's Antwort.
        antwort_code: String,
    },
    /// Process rejected (structural validation failure or timeout).
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// GeLi Gas 3.0 Kap. 4.3 bounds the Stammdatenänderung by Fristen „die insgesamt 10 Werktage nicht überschreiten", and states no consequence of missing them. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GasStammdatenState>,
    },
}

/// A Stammdatenänderung holds its Marktlokation from the first message until
/// it is answered or refused: the ingest door keys a second Änderung on the
/// same Marktlokation by the same MaLo, and the command doors find the process
/// they answer by it.
impl mako_engine::workflow::OccupiesBusinessKey for GasStammdatenState {
    fn occupies_business_key(&self) -> bool {
        !matches!(self, Self::New) && !self.is_terminal()
    }
}

impl GasStammdatenState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::AnfrageOffen(_) => "AnfrageOffen",
            Self::AnfrageBeantwortet { .. } => "AnfrageBeantwortet",
            Self::Gesendet(_) => "Gesendet",
            Self::Abgeschlossen { .. } => "Abgeschlossen",
            Self::Rejected { .. } => "Rejected",
            Self::FristAbgelaufen { .. } => "FristAbgelaufen",
        }
    }

    /// The state the process is waiting in — itself, unless a window lapsed.
    #[must_use]
    pub fn offen(&self) -> &Self {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.offen(),
            other => other,
        }
    }

    /// `true` when terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. }
                | Self::AnfrageBeantwortet { .. }
                | Self::Abgeschlossen { .. }
                | Self::Rejected { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas Stammdatenänderung workflow.
#[derive(Clone)]
pub enum GasStammdatenCommand {
    /// Berechtigter: inbound UTILMD G Änderung received.
    ReceiveAenderung {
        /// Änderung PID.
        pid: Pruefidentifikator,
        /// GLN of the Verantwortlicher.
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Änderungsdatum (`YYYYMMDD`).
        aenderungsdatum: String,
        /// The applied MaLo attribute patch (JSON; only for MaLo objects).
        patch: serde_json::Value,
        /// `SG4 DTM+92` Datum Vertragsbeginn (`YYYYMMDD`).
        vertragsbeginn: Option<String>,
        /// The Änderung's `IDE+24` Vorgangsnummer.
        vorgangsnummer: Option<String>,
        /// The Änderung's `BGM` DE 1001.
        dokumentenart: Option<String>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if structural validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
        /// Receipt timestamp (drives the APERAK + Antwort deadlines).
        received_at: time::OffsetDateTime,
    },
    /// Berechtigter: send the Antwort (Zustimmung or Ablehnung).
    ///
    /// The code is drawn from the Codeliste the Antwort column names
    /// ([`antwort_codeliste`]) and refused when that list does not publish
    /// it. A Zustimmung to a bilanzierungsrelevante change dated other than a
    /// Monatserster goes out as [`ABLEHNUNG_FRIST`].
    SendAntwort {
        /// `SG4 STS+E01` DE 9013 — `E15`, or an Ablehnungscode of the column's
        /// Codeliste.
        antwort_code: String,
    },
    /// Verantwortlicher: dispatch an Änderung (ERP-initiated, e.g. LF → NB).
    SendAenderung {
        /// Änderung PID.
        pid: Pruefidentifikator,
        /// GLN of the Verantwortlicher (us).
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Gas-Marktlokation.
        location_id: MaLo,
        /// Änderungsdatum (`YYYYMMDD`) — `SG4 DTM+157`.
        aenderungsdatum: String,
        /// Datum Vertragsbeginn (`YYYYMMDD`) — `SG4 DTM+92`, Muss beside the
        /// `157` on every Änderung column (UTILMD AHB Gas 1.2 Kap. 5.10–5.11,
        /// Nr 00014 and 00017).
        vertragsbeginn: String,
        /// The changed Stammdaten. Every data group of an Änderung column is
        /// Soll `[92]` „Wenn Wert innerhalb SG bzw. Segment geändert wird"
        /// (UTILMD AHB Gas 1.2 Kap. 5.10.1, S. 97–110; Kap. 5.11.3, S.
        /// 120–123), so an Änderung carries the groups that changed and is
        /// refused carrying none.
        stammdaten: Stammdaten,
    },
    /// Verantwortlicher: the Berechtigter's Antwort arrived.
    ReceiveAntwort {
        /// Antwort PID.
        response_pid: Pruefidentifikator,
        /// `SG4 STS+E01` DE 9013.
        antwort_code: String,
        /// `SG4 STS+E01` DE 1131.
        antwort_codeliste: Option<String>,
    },
    /// Verantwortlicher: an Anfrage zur Stammdatenänderung arrived. Opens the
    /// Antwort window; the answer is [`Self::SendAnfrageAntwort`].
    ReceiveAnfrage {
        /// Anfrage PID.
        pid: Pruefidentifikator,
        /// MP-ID of the Berechtigter (requester).
        sender: MarktpartnerCode,
        /// MP-ID of the Verantwortlicher (us).
        receiver: MarktpartnerCode,
        /// The Meldepunkt the Anfrage names.
        location_id: MaLo,
        /// `SG4 DTM+157` (`YYYYMMDD`).
        aenderungsdatum: String,
        /// `SG4 DTM+92` (`YYYYMMDD`).
        vertragsbeginn: Option<String>,
        /// `SG4 STS+7` Transaktionsgrund.
        transaktionsgrund: Option<String>,
        /// The Anfrage's `IDE+24`.
        vorgangsnummer: Option<String>,
        /// The groups the Anfrage filled.
        angefragt: Vec<AngefragteGruppe>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if structural validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
        /// Receipt timestamp (drives the APERAK + Antwort deadlines).
        received_at: time::OffsetDateTime,
    },
    /// Verantwortlicher: answer the open Anfrage zur Stammdatenänderung.
    ///
    /// The Stammdaten are read at the boundary — the workflow places them; it
    /// reads nothing.
    ///
    /// The code picks the Antwort column ([`anfrage_antwort`]) — its PID and
    /// the Codeliste `SG4 STS+E01` DE 1131 names.
    SendAnfrageAntwort {
        /// `SG4 STS+E01` DE 9013 — the code of the check's result.
        antwort_code: String,
        /// The `SG5`/`SG6`/`SG8`/`SG12` groups the Antwort carries. Only a
        /// [`DATEN_CODES`] code carries `SG6`/`SG8`/`SG12`; `SG5` names the
        /// Messlokationen where the column asks for them (`[590]`, Kap. 5.12.3).
        stammdaten: Stammdaten,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for GasStammdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// Build the `ProcessCompleted` outbox payload that drives the marktd apply
/// (only on Zustimmung of a MaLo change).
fn apply_outbox(data: &GasStammdatenData, patch: &serde_json::Value) -> PendingOutbox {
    PendingOutbox::new(
        "ProcessCompleted",
        "",
        serde_json::json!({
            "pid":              data.pruefidentifikator.as_u32(),
            "malo_id":          data.location_id.as_str(),
            "objekt":           "MARKTLOKATION",
            "aenderungsdatum":  data.aenderungsdatum,
            "stammdaten_patch": patch,
        }),
    )
}

/// GeLi Gas Stammdatenänderung workflow (PIDs 44109–44182).
pub struct GeliGasStammdatenaenderungWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// The answer window is open while an Antwort is owed: by this party
/// (`Eingegangen`, `ValidationPassed`, `AnfrageOffen`) or to it (`Gesendet`).
fn window_open(label: &str, state: &GasStammdatenState) -> bool {
    label == ANTWORT_WINDOW_LABEL
        && matches!(
            state,
            GasStammdatenState::Eingegangen(_)
                | GasStammdatenState::ValidationPassed(_)
                | GasStammdatenState::AnfrageOffen(_)
                | GasStammdatenState::Gesendet(_)
        )
}

/// `ZG2` „Gültiges Ergebnis nach der Datenprüfung": every data group of the
/// 44142 column is conditioned on „Wenn SG4 STS+E01+ZG2 … vorhanden" (UTILMD
/// AHB Gas 1.2 Kap. 5.12.1, Bedingung `[202]`).
pub const GUELTIGES_ERGEBNIS: &str = "ZG2";

impl Workflow for GeliGasStammdatenaenderungWorkflow {
    type State = GasStammdatenState;
    type Event = GasStammdatenEvent;
    type Command = GasStammdatenCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GasStammdatenCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            GasStammdatenState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GasStammdatenEvent::AenderungErhalten {
                location_id,
                sender,
                receiver,
                pruefidentifikator,
                bilanzierungsrelevant,
                aenderungsdatum,
                patch,
                vertragsbeginn,
                vorgangsnummer,
                dokumentenart,
                ..
            } => GasStammdatenState::Eingegangen(GasStammdatenData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
                bilanzierungsrelevant: *bilanzierungsrelevant,
                aenderungsdatum: aenderungsdatum.clone(),
                patch: patch.clone(),
                vertragsbeginn: vertragsbeginn.clone(),
                vorgangsnummer: vorgangsnummer.clone(),
                dokumentenart: dokumentenart.clone(),
            }),
            GasStammdatenEvent::ValidationPassed { .. } => match state {
                GasStammdatenState::Eingegangen(data) => GasStammdatenState::ValidationPassed(data),
                other => other,
            },
            GasStammdatenEvent::AntwortGesendet { antwort_code, .. } => match state {
                GasStammdatenState::ValidationPassed(data) => GasStammdatenState::Beantwortet {
                    data,
                    antwort_code: antwort_code.clone(),
                },
                other => other,
            },
            GasStammdatenEvent::AenderungGesendet {
                location_id,
                sender,
                receiver,
                pruefidentifikator,
                bilanzierungsrelevant,
                aenderungsdatum,
                vertragsbeginn,
            } => GasStammdatenState::Gesendet(GasStammdatenData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
                bilanzierungsrelevant: *bilanzierungsrelevant,
                aenderungsdatum: aenderungsdatum.clone(),
                patch: serde_json::Value::Null,
                vertragsbeginn: vertragsbeginn.clone(),
                vorgangsnummer: None,
                dokumentenart: Some(AENDERUNGSMELDUNGEN.to_owned()),
            }),
            GasStammdatenEvent::AntwortErhalten { antwort_code, .. } => match state {
                GasStammdatenState::Gesendet(data) => GasStammdatenState::Abgeschlossen {
                    data,
                    antwort_code: antwort_code.clone(),
                },
                other => other,
            },
            GasStammdatenEvent::AnfrageErhalten { anfrage, .. } => {
                GasStammdatenState::AnfrageOffen(anfrage.clone())
            }
            GasStammdatenEvent::AnfrageBeantwortet {
                location_id,
                anfrage_pid,
                antwort_pid,
                antwort_code,
                ..
            } => GasStammdatenState::AnfrageBeantwortet {
                location_id: location_id.clone(),
                anfrage_pid: *anfrage_pid,
                antwort_pid: *antwort_pid,
                antwort_code: antwort_code.clone(),
            },
            GasStammdatenEvent::Rejected { reason } => GasStammdatenState::Rejected {
                reason: reason.clone(),
            },
            GasStammdatenEvent::DeadlineExpired { label, .. } => match state {
                GasStammdatenState::Eingegangen(_)
                | GasStammdatenState::ValidationPassed(_)
                | GasStammdatenState::AnfrageOffen(_)
                | GasStammdatenState::Gesendet(_) => GasStammdatenState::FristAbgelaufen {
                    label: label.to_string(),
                    offen: Box::new(state),
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            // ── Berechtigter: inbound Änderung ───────────────────────────────
            GasStammdatenCommand::ReceiveAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
                patch,
                vertragsbeginn,
                vorgangsnummer,
                dokumentenart,
                message_ref,
                validation_passed,
                validation_errors,
                received_at,
            } => {
                if !matches!(state, GasStammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some((_, bilanzierungsrelevant)) = antwort_for(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a GeLi Gas Änderung PID",
                    )));
                };
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![GasStammdatenEvent::AenderungErhalten {
                    location_id,
                    sender,
                    receiver,
                    pruefidentifikator: pid,
                    bilanzierungsrelevant,
                    aenderungsdatum: aenderungsdatum.clone(),
                    patch: patch.clone(),
                    message_ref: message_ref.clone(),
                    vertragsbeginn,
                    vorgangsnummer,
                    dokumentenart,
                }];

                if !validation_passed {
                    let reason = if validation_errors.is_empty() {
                        "structural validation failed".to_owned()
                    } else {
                        validation_errors.join("; ")
                    };
                    events.push(GasStammdatenEvent::Rejected {
                        reason: reason.clone(),
                    });
                    let outbox = vec![
                        PendingOutbox::aperak_fehler(
                            receiver_gln.as_str(),
                            sender_mp_id.as_str(),
                            message_ref.as_str(),
                            mako_engine::erc::codes::Z29,
                            reason,
                        )
                        .caused_by(0),
                    ];
                    return Ok(WorkflowOutput::with_outbox(events, outbox));
                }

                events.push(GasStammdatenEvent::ValidationPassed {
                    message_ref: message_ref.clone(),
                });
                // No APERAK: in der Sparte Gas the APERAK carries only the
                // Verarbeitbarkeitsfehler (APERAK AHB 1.1 §2.3); processability
                // follows from the Frist passing without one. So no APERAK
                // delivery window either — it would watch a message that never
                // goes out.
                let deadlines = vec![PendingDeadline::new(
                    ANTWORT_WINDOW_LABEL,
                    deadline_at_werktage(
                        received_at,
                        mako_fristen::antwort::STAMMDATEN_ANTWORT_WERKTAGE_GAS,
                        HolidayCalendar::BdewMaKo,
                    ),
                )];
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    events,
                    Vec::new(),
                    deadlines,
                ))
            }

            // ── Verantwortlicher: inbound Anfrage zur Stammdatenänderung ──────
            GasStammdatenCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
                vertragsbeginn,
                transaktionsgrund,
                vorgangsnummer,
                angefragt,
                message_ref,
                validation_passed,
                validation_errors,
                received_at,
            } => {
                if !matches!(state, GasStammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some(antwort_code) = antwort_for_anfrage(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a GeLi Gas Stammdatenanfrage PID",
                    )));
                };
                let antwort_pid = Pruefidentifikator::new(antwort_code)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;

                if !validation_passed {
                    let reason = if validation_errors.is_empty() {
                        "structural validation failed".to_owned()
                    } else {
                        validation_errors.join("; ")
                    };
                    let outbox = vec![
                        PendingOutbox::aperak_fehler(
                            receiver.as_str(),
                            sender.as_str(),
                            message_ref.as_str(),
                            mako_engine::erc::codes::Z29,
                            reason.clone(),
                        )
                        .caused_by(0),
                    ];
                    return Ok(WorkflowOutput::with_outbox(
                        vec![GasStammdatenEvent::Rejected { reason }],
                        outbox,
                    ));
                }

                // Nothing goes out yet: the Verantwortlicher checks the Anfrage
                // first and answers „Unverzüglich, spätestens jedoch bis zum
                // Ablauf des 10. WT nach Eingang der Nachricht" (AWH GeLi Gas
                // V1.2 Kap. 4.4.2 Nr. 2, S. 87). No APERAK either — a Gas APERAK
                // is a Verarbeitbarkeitsfehlermeldung only (APERAK AHB 1.1 §2.3).
                let deadlines = vec![PendingDeadline::new(
                    ANTWORT_WINDOW_LABEL,
                    deadline_at_werktage(
                        received_at,
                        mako_fristen::antwort::STAMMDATEN_ANTWORT_WERKTAGE_GAS,
                        HolidayCalendar::BdewMaKo,
                    ),
                )];
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![GasStammdatenEvent::AnfrageErhalten {
                        anfrage: GasAnfrageData {
                            location_id,
                            requester: sender,
                            owner: receiver,
                            anfrage_pid: pid,
                            antwort_pid,
                            aenderungsdatum,
                            vertragsbeginn,
                            transaktionsgrund,
                            vorgangsnummer,
                            angefragt,
                        },
                        message_ref,
                    }],
                    Vec::new(),
                    deadlines,
                ))
            }

            // ── Verantwortlicher: the Antwort auf Anfrage ─────────────────────
            GasStammdatenCommand::SendAnfrageAntwort {
                antwort_code,
                stammdaten,
            } => {
                let GasStammdatenState::AnfrageOffen(a) = state else {
                    return Err(WorkflowError::invalid_state("AnfrageOffen", state.label()));
                };
                let anfrage_pid = a.anfrage_pid.as_u32();
                let Some((spalte, _)) = anfrage_antwort(anfrage_pid, antwort_code.trim()) else {
                    let admitted: Vec<String> = anfrage_antworten(anfrage_pid)
                        .map(|r| format!("{} on {}", r.codeliste, r.antwort_pid))
                        .collect();
                    return Err(WorkflowError::validation(if admitted.is_empty() {
                        format!(
                            "the Antwort to an Anfrage {anfrage_pid} is not assembled \
                             (UTILMD AHB Gas 1.2 Kap. 5.12)"
                        )
                    } else {
                        format!(
                            "Antwortcode {antwort_code:?} is admitted by no Antwort column of \
                             the Anfrage {anfrage_pid} — {} (UTILMD AHB Gas 1.2 Kap. 5.12, \
                             SG4 STS 1131)",
                            admitted.join("; ")
                        )
                    }));
                };
                let traegt_daten = DATEN_CODES.contains(&antwort_code.trim());
                let datengruppen = !stammdaten.sg6.is_empty()
                    || !stammdaten.bloecke.is_empty()
                    || !stammdaten.sg12.is_empty();
                if datengruppen && !traegt_daten {
                    return Err(WorkflowError::validation(format!(
                        "Antwortcode {antwort_code} carries no Stammdaten — the data groups \
                         of an Antwort auf Anfrage are conditioned on STS+E01+ZG2 (UTILMD AHB \
                         Gas 1.2 Kap. 5.12, Bedingung [202]), and only ZG2/ZG4 transmit \
                         „die … gültigen Stammdaten“ (EBD 4.3 Kap. 13.13.1)"
                    )));
                }
                let antwort_pid = Pruefidentifikator::new(spalte.antwort_pid)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                let antwort_codeliste = spalte.codeliste.to_owned();
                // `[528]` „Es ist das Datum/ Daten aus der Anfrage zu
                // verwenden" — where the column lists the two dates at all.
                let (process_date, vertragsbeginn) = if spalte.sg4_daten {
                    (Some(a.aenderungsdatum.clone()), a.vertragsbeginn.clone())
                } else {
                    (None, None)
                };
                let outbox = vec![PendingOutbox::typed(
                    a.requester.as_str(),
                    &UtilmdPayload {
                        pid: spalte.antwort_pid,
                        sender: a.owner.as_str().to_owned(),
                        receiver: Some(a.requester.as_str().to_owned()),
                        malo: Some(a.location_id.as_str().to_owned()),
                        process_date,
                        vertragsbeginn,
                        transaktionsgrund: a.transaktionsgrund.clone(),
                        referenz_vorgangsnummer: a.vorgangsnummer.clone(),
                        antwort_code: Some(antwort_code.trim().to_owned()),
                        antwort_codeliste: Some(antwort_codeliste.clone()),
                        stammdaten,
                        ..UtilmdPayload::default()
                    },
                )];
                Ok(WorkflowOutput::with_outbox(
                    vec![GasStammdatenEvent::AnfrageBeantwortet {
                        location_id: a.location_id.clone(),
                        anfrage_pid: a.anfrage_pid,
                        antwort_pid,
                        antwort_code: antwort_code.trim().to_owned(),
                        antwort_codeliste,
                    }],
                    outbox,
                ))
            }

            GasStammdatenCommand::SendAntwort { antwort_code } => {
                let data = match state {
                    GasStammdatenState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                let aenderung = data.pruefidentifikator.as_u32();
                let (Some((response_code, _)), Some(codeliste)) =
                    (antwort_for(aenderung), antwort_codeliste(aenderung))
                else {
                    return Err(WorkflowError::rejected(format!(
                        "no Antwort PID or Codeliste for Änderung {aenderung}"
                    )));
                };
                let response_pid = Pruefidentifikator::new(response_code)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                // Monatserster guard: a bila.rel. change dated other than the
                // first of a month is refused `E17`, never accepted — „Bei der
                // Übermittlung von bilanzierungsrelevanten Stammdatenänderungen
                // wird auch eine Ablehnung erfolgen, wenn das Änderungsdatum
                // kein Monatserster ist." (EBD 4.3 Kap. 13.10.1.2 / 13.11.1.2).
                let antwort_code = if data.bilanzierungsrelevant
                    && antwort_code == ZUSTIMMUNG
                    && !is_monatserster(&data.aenderungsdatum)
                {
                    ABLEHNUNG_FRIST.to_owned()
                } else {
                    antwort_code
                };
                // The code comes from the column's own Codeliste: `G_0016`
                // publishes `E15` alone, so a 44115 has no Ablehnung to send.
                let Some(published) = mako_pruefung::codes::lookup(codeliste, &antwort_code) else {
                    let alphabet: Vec<&str> = mako_pruefung::codes::CODELISTEN
                        .iter()
                        .find(|(id, _)| *id == codeliste)
                        .map(|(_, codes)| codes.iter().map(|c| c.code).collect())
                        .unwrap_or_default();
                    return Err(WorkflowError::validation(format!(
                        "Antwortcode {antwort_code} is not in Codeliste {codeliste}, which the \
                         {response_code} column names (UTILMD AHB Gas 1.2 Kap. 5.10–5.11); it \
                         publishes {}",
                        alphabet.join(", ")
                    )));
                };
                let zustimmung = published.ist_zustimmung() == Some(true);

                // `SG4 DTM` only where the column lists it — the 44176 restates
                // both dates of the Änderung it answers, no other Antwort names
                // one; the renderer asks the column. `STS+7` is the one code the
                // column fixes.
                let mut outbox = vec![PendingOutbox::typed(
                    data.sender.as_str(),
                    &UtilmdPayload {
                        pid: response_pid.as_u32(),
                        sender: data.receiver.as_str().to_owned(),
                        receiver: Some(data.sender.as_str().to_owned()),
                        malo: Some(data.location_id.as_str().to_owned()),
                        process_date: Some(data.aenderungsdatum.clone()),
                        vertragsbeginn: data.vertragsbeginn.clone(),
                        document_code: data.dokumentenart.clone(),
                        referenz_vorgangsnummer: data.vorgangsnummer.clone(),
                        antwort_code: Some(antwort_code.clone()),
                        antwort_codeliste: Some(codeliste.to_owned()),
                        ..UtilmdPayload::default()
                    },
                )];
                // Apply to marktd only on Zustimmung (the patch captured at
                // ingest). Ablehnung → no apply.
                if zustimmung && data.patch.as_object().is_some_and(|o| !o.is_empty()) {
                    outbox.push(apply_outbox(data, &data.patch));
                }
                Ok(WorkflowOutput::with_outbox(
                    vec![GasStammdatenEvent::AntwortGesendet {
                        response_pid,
                        antwort_code,
                        antwort_codeliste: codeliste.to_owned(),
                    }],
                    outbox,
                ))
            }

            // ── Verantwortlicher: outbound Änderung ──────────────────────────
            GasStammdatenCommand::SendAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
                vertragsbeginn,
                stammdaten,
            } => {
                if !matches!(state, GasStammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some((_, bilanzierungsrelevant)) = antwort_for(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a GeLi Gas Änderung PID",
                    )));
                };
                if vertragsbeginn.trim().is_empty() {
                    return Err(WorkflowError::validation(
                        "an Änderung carries SG4 DTM+92 Datum Vertragsbeginn beside DTM+157 \
                         (UTILMD AHB Gas 1.2 Kap. 5.10–5.11, Muss)"
                            .to_owned(),
                    ));
                }
                if stammdaten.is_empty() {
                    return Err(WorkflowError::validation(format!(
                        "an Änderung {pid} names no changed Stammdatum — each data group is \
                         sent „Wenn Wert innerhalb SG bzw. Segment geändert wird“ (UTILMD AHB \
                         Gas 1.2 Kap. 5.10–5.11, Bedingung [92])"
                    )));
                }
                // A bila.rel. change must carry a Monatserster Änderungsdatum.
                if bilanzierungsrelevant && !is_monatserster(&aenderungsdatum) {
                    return Err(WorkflowError::rejected(format!(
                        "bilanzierungsrelevante Änderung requires a Monatserster \
                         Änderungsdatum (YYYYMM01), got {aenderungsdatum}",
                    )));
                }
                let utilmd = PendingOutbox::typed(
                    receiver.as_str(),
                    &UtilmdPayload {
                        pid: pid.as_u32(),
                        sender: sender.as_str().to_owned(),
                        receiver: Some(receiver.as_str().to_owned()),
                        malo: Some(location_id.as_str().to_owned()),
                        process_date: Some(aenderungsdatum.clone()),
                        vertragsbeginn: Some(vertragsbeginn.clone()),
                        document_code: Some(AENDERUNGSMELDUNGEN.to_owned()),
                        stammdaten,
                        ..UtilmdPayload::default()
                    },
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![GasStammdatenEvent::AenderungGesendet {
                        location_id,
                        sender,
                        receiver,
                        pruefidentifikator: pid,
                        bilanzierungsrelevant,
                        aenderungsdatum,
                        vertragsbeginn: Some(vertragsbeginn),
                    }],
                    vec![utilmd],
                ))
            }

            GasStammdatenCommand::ReceiveAntwort {
                response_pid,
                antwort_code,
                antwort_codeliste,
            } => {
                if !matches!(state, GasStammdatenState::Gesendet(_)) {
                    return Err(WorkflowError::invalid_state("Gesendet", state.label()));
                }
                if !is_antwort_pid(response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {response_pid} is not a GeLi Gas Antwort PID",
                    )));
                }
                Ok(vec![GasStammdatenEvent::AntwortErhalten {
                    response_pid,
                    antwort_code,
                    antwort_codeliste,
                }]
                .into())
            }

            GasStammdatenCommand::TimeoutExpired { deadline_id, label } => {
                if window_open(&label, lapsed) {
                    Ok(vec![GasStammdatenEvent::DeadlineExpired { deadline_id, label }].into())
                } else {
                    Ok(vec![].into())
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::payload::OutboundPayload as _;
    use mako_engine::{ids::DeadlineId, workflow::Workflow};

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }
    fn now() -> time::OffsetDateTime {
        time::macros::datetime!(2026-07-01 10:00:00 UTC)
    }

    #[test]
    fn verlauf_states_the_messages() {
        let aenderung = GasStammdatenEvent::AenderungErhalten {
            location_id: malo("51238696012"),
            sender: mcod("9800357000004"),
            receiver: mcod("9800357000011"),
            pruefidentifikator: pid(44112),
            bilanzierungsrelevant: false,
            aenderungsdatum: "20260801".to_owned(),
            patch: serde_json::json!({}),
            message_ref: mref("GSD-001"),
            vertragsbeginn: Some("20250101".to_owned()),
            vorgangsnummer: Some("VG-1".to_owned()),
            dokumentenart: Some("E03".to_owned()),
        };
        assert_eq!(
            aenderung.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(44112),
                message_ref: Some("GSD-001".to_owned()),
            })
        );
        let antwort = GasStammdatenEvent::AntwortGesendet {
            response_pid: pid(44115),
            antwort_code: "E15".to_owned(),
            antwort_codeliste: "G_0016".to_owned(),
        };
        // The Antwort auf Änderung states its code on the envelope too.
        assert_eq!(
            antwort.antwort(),
            Some(mako_engine::envelope::Antwortcode::new(
                "E15",
                Some("G_0016")
            ))
        );
        assert_eq!(
            antwort.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(44115),
                message_ref: None,
            })
        );
        let beantwortet = GasStammdatenEvent::AnfrageBeantwortet {
            location_id: malo("51238696012"),
            anfrage_pid: pid(44139),
            antwort_pid: pid(44142),
            antwort_code: "ZG2".to_owned(),
            antwort_codeliste: "G_0031".to_owned(),
        };
        assert_eq!(
            beantwortet.antwort(),
            Some(mako_engine::envelope::Antwortcode::new(
                "ZG2",
                Some("G_0031")
            ))
        );
        assert_eq!(
            beantwortet.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(44142),
                message_ref: None,
            })
        );
        // The APERAK 313 queued beside the rejection.
        let rejected = GasStammdatenEvent::Rejected {
            reason: "x".to_owned(),
        };
        assert_eq!(
            rejected.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: None,
                message_ref: None,
            })
        );
    }

    fn receive_cmd(aenderung_pid: u32, datum: &str, ok: bool) -> GasStammdatenCommand {
        GasStammdatenCommand::ReceiveAenderung {
            pid: pid(aenderung_pid),
            sender: mcod("9800357000004"),
            receiver: mcod("9800357000011"),
            location_id: malo("51238696781"),
            aenderungsdatum: datum.to_owned(),
            patch: serde_json::json!({ "gasqualitaet": "HGAS" }),
            vertragsbeginn: Some("20250101".to_owned()),
            vorgangsnummer: Some("GSD-VG-1".to_owned()),
            dokumentenart: Some("E03".to_owned()),
            message_ref: mref("GSD-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            received_at: now(),
        }
    }

    fn apply_all(init: GasStammdatenState, events: &[GasStammdatenEvent]) -> GasStammdatenState {
        events
            .iter()
            .fold(init, GeliGasStammdatenaenderungWorkflow::apply)
    }

    #[test]
    fn classification() {
        assert_eq!(antwort_for(44112), Some((44115, false)));
        assert_eq!(antwort_for(44123), Some((44124, true))); // bila.rel.
        assert!(is_aenderung_pid(44109));
        assert!(is_antwort_pid(44115));
        // WiM Gas Verpflichtungsanfrage is excluded.
        assert!(!is_aenderung_pid(44168));
        assert!(antwort_for(44168).is_none());
    }

    #[test]
    fn no_duplicate_aenderung_pids() {
        let mut seen = std::collections::HashSet::new();
        for (a, _, _) in STAMMDATEN_PAIRS {
            assert!(seen.insert(*a), "duplicate Änderung PID {a}");
        }
        // Anfrage PIDs must not overlap the change families.
        for a in ANFRAGE_PIDS {
            assert!(!is_aenderung_pid(*a) && !is_antwort_pid(*a), "overlap {a}");
        }
    }

    #[test]
    fn nicht_bila_change_accepted_e15() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort_code: "E15".to_owned(),
            },
        )
        .unwrap();
        let antwort = UtilmdPayload::from_payload(&out.outbox[0].payload).expect("typed");
        assert_eq!(antwort.pid, 44115);
        assert_eq!(antwort.antwort_code.as_deref(), Some("E15"));
        assert_eq!(antwort.antwort_codeliste.as_deref(), Some("G_0016"));
        // `SG6 RFF+TN` names the Änderung's Vorgang.
        assert_eq!(antwort.referenz_vorgangsnummer.as_deref(), Some("GSD-VG-1"));
        // Zustimmung → apply outbox present.
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[1].message_type.as_ref(), "ProcessCompleted");
    }

    /// A processable Gas Änderung is answered by no APERAK.
    ///
    /// APERAK AHB 1.1 §2.3: „In der Sparte Gas wird dem Absender eines
    /// Geschäftsvorfalls … nur eines der beiden Ergebnisse … via APERAK
    /// mitgeteilt: Das Ergebnis, dass der geprüfte Geschäftsvorfall einen
    /// Verarbeitbarkeitsfehler enthält." Processability follows from the
    /// Frist passing without one, so an APERAK 29002 (`BGM+312`) here is wire
    /// output the AHB does not allow.
    #[test]
    fn a_valid_aenderung_enqueues_no_aperak() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", true),
        )
        .unwrap();
        assert!(
            out.outbox
                .iter()
                .all(|o| o.message_type.as_ref() != "APERAK"),
            "a Gas Anerkennungsmeldung was enqueued: {:?}",
            out.outbox
                .iter()
                .map(|o| (o.message_type.as_ref(), o.payload["pid"].clone()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn bila_change_non_monatserster_rejected_e17() {
        // 44123 is bila.rel.; dated 2026-07-15 (not a Monatserster) → E17 even
        // when the operator asks for Zustimmung.
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44123, "20260715", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort_code: "E15".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["antwort_code"], "E17");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "G_0019");
        // Ablehnung → no apply outbox.
        assert_eq!(out.outbox.len(), 1);
    }

    #[test]
    fn bila_change_monatserster_accepted() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44123, "20260801", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort_code: "E15".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["antwort_code"], "E15");
    }

    /// `E13` is in `G_0025`, the Codeliste of the 44121 — the NB's answer to
    /// a bilanzierungsrelevante Änderung vom LF (EBD 4.3 Kap. 13.11.1.2).
    #[test]
    fn explicit_ablehnung_bilanzierung_e13() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44120, "20260801", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort_code: "E13".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pid"], 44121);
        assert_eq!(out.outbox[0].payload["antwort_code"], "E13");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "G_0025");
        assert_eq!(out.outbox.len(), 1); // no apply on Ablehnung
    }

    /// Each Antwort column names its own Codeliste. `G_0016` — the 44115's —
    /// publishes `E15` alone (EBD 4.3 Kap. 13.10.1.1), so an `E13` there is a
    /// code the receiver's alphabet does not have, and it is refused before
    /// anything is queued.
    #[test]
    fn a_code_outside_the_columns_codeliste_is_refused() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", true),
        )
        .unwrap();
        let state = apply_all(GasStammdatenState::New, &out.events);
        let err = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::SendAntwort {
                antwort_code: "E13".to_owned(),
            },
        )
        .expect_err("G_0016 has no E13");
        assert!(err.to_string().contains("G_0016"), "{err}");
    }

    /// Every Änderung has an Antwort Codeliste, and every one of them is
    /// catalogued and publishes the Zustimmung the auto-path answers with.
    #[test]
    fn every_aenderung_names_a_catalogued_codeliste() {
        for (aenderung, _, bila) in STAMMDATEN_PAIRS {
            let cl = antwort_codeliste(*aenderung)
                .unwrap_or_else(|| panic!("{aenderung} has no Antwort Codeliste"));
            assert!(
                mako_pruefung::codes::lookup(cl, ZUSTIMMUNG).is_some(),
                "{cl}"
            );
            if *bila {
                assert!(
                    mako_pruefung::codes::lookup(cl, ABLEHNUNG_FRIST).is_some(),
                    "{cl} must publish the Monatserster refusal"
                );
            }
        }
        assert_eq!(ANTWORT_CODELISTEN.len(), STAMMDATEN_PAIRS.len());
    }

    /// The 44119 column names two Codelisten by the sender's role: the NB
    /// answers the MSB's 44116 from `G_0026`, the LF the NB's forwarded 44117
    /// from `G_0027` (UTILMD AHB Gas 1.2 Kap. 5.11.5, `[98]` / `[5]`).
    #[test]
    fn the_44119_codeliste_follows_who_answers() {
        assert_eq!(antwort_codeliste(44116), Some("G_0026"));
        assert_eq!(antwort_codeliste(44117), Some("G_0027"));
        assert_eq!(antwort_codeliste(44159), Some("G_0026"));
        assert_eq!(antwort_codeliste(44160), Some("G_0030"));
    }

    /// An Änderung naming no changed group transmits nothing: every data group
    /// of its column is sent „Wenn Wert … geändert wird“ (`[92]`).
    #[test]
    fn an_aenderung_without_a_changed_group_is_refused() {
        let err = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            GasStammdatenCommand::SendAenderung {
                pid: pid(44109),
                sender: mcod("9800357000011"),
                receiver: mcod("9800357000004"),
                location_id: malo("51238696012"),
                aenderungsdatum: "20260715".to_owned(),
                vertragsbeginn: "20250101".to_owned(),
                stammdaten: Stammdaten::default(),
            },
        )
        .expect_err("an empty Änderung is refused");
        assert!(err.to_string().contains("[92]"), "{err}");
    }

    #[test]
    fn initiator_bila_requires_monatserster() {
        let result = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            GasStammdatenCommand::SendAenderung {
                pid: pid(44120), // bila.rel. LF → NB
                sender: mcod("9800357000011"),
                receiver: mcod("9800357000004"),
                location_id: malo("51238696781"),
                aenderungsdatum: "20260715".to_owned(), // not Monatserster
                vertragsbeginn: "20250101".to_owned(),
                stammdaten: gemeinderabatt(),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn initiator_send_receive_roundtrip() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            GasStammdatenCommand::SendAenderung {
                pid: pid(44109), // nicht-bila LF → NB
                sender: mcod("9800357000011"),
                receiver: mcod("9800357000004"),
                location_id: malo("51238696781"),
                aenderungsdatum: "20260715".to_owned(),
                vertragsbeginn: "20250101".to_owned(),
                stammdaten: gemeinderabatt(),
            },
        )
        .unwrap();
        // Both dates the Änderung column makes Muss: DTM+92 and DTM+157.
        let aenderung = UtilmdPayload::from_payload(&out.outbox[0].payload).expect("typed");
        assert_eq!(aenderung.vertragsbeginn.as_deref(), Some("20250101"));
        assert_eq!(aenderung.process_date.as_deref(), Some("20260715"));
        let state = apply_all(GasStammdatenState::New, &out.events);
        assert!(matches!(state, GasStammdatenState::Gesendet(_)));
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::ReceiveAntwort {
                response_pid: pid(44111),
                antwort_code: "E15".to_owned(),
                antwort_codeliste: Some("G_0023".to_owned()),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasStammdatenState::Abgeschlossen { .. }));
    }

    #[test]
    fn validation_failure_aperak_313() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", false),
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
        let state = apply_all(GasStammdatenState::New, &out.events);
        assert!(matches!(state, GasStammdatenState::Rejected { .. }));
    }

    /// The Berechtigter's own Antwort window lapsing is a Frist breach, not a
    /// rejection — and a processable Änderung arms no APERAK window to lapse.
    #[test]
    fn antwort_timeout_is_a_frist_breach() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            receive_cmd(44112, "20260715", true),
        )
        .unwrap();
        assert!(
            out.deadlines
                .iter()
                .all(|d| !mako_fristen::is_delivery_window(&d.label)),
            "{:?}",
            out.deadlines
        );
        let state = apply_all(GasStammdatenState::New, &out.events);
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &state,
            GasStammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasStammdatenState::FristAbgelaufen { .. }));
    }

    #[test]
    fn monatserster_helper() {
        assert!(is_monatserster("20260801"));
        assert!(!is_monatserster("20260815"));
        assert!(!is_monatserster("2026080")); // wrong length
    }

    fn anfrage_cmd(anfrage_pid: u32, ok: bool) -> GasStammdatenCommand {
        GasStammdatenCommand::ReceiveAnfrage {
            pid: pid(anfrage_pid),
            sender: mcod("9800357000011"),   // requester (Berechtigter)
            receiver: mcod("9800357000004"), // us (Verantwortlicher)
            location_id: malo("51238696781"),
            aenderungsdatum: "20260801".to_owned(),
            vertragsbeginn: Some("20260101".to_owned()),
            transaktionsgrund: Some("ZF4".to_owned()),
            vorgangsnummer: Some("ANF-VG-1".to_owned()),
            angefragt: vec![
                AngefragteGruppe::Marktgebiet,
                AngefragteGruppe::Gemeinderabatt,
            ],
            message_ref: mref("GSA-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            received_at: now(),
        }
    }

    fn offen() -> GasStammdatenState {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            anfrage_cmd(44139, true),
        )
        .unwrap();
        apply_all(GasStammdatenState::New, &out.events)
    }

    fn gemeinderabatt() -> Stammdaten {
        Stammdaten {
            bloecke: vec![mako_engine::payload::StammdatenBlock {
                seq: "Z12".to_owned(),
                segmente: vec![mako_engine::payload::StammdatenSegment {
                    tag: "QTY".to_owned(),
                    elemente: vec![vec!["Z16".to_owned(), "3".to_owned(), "P1".to_owned()]],
                }],
            }],
            ..Stammdaten::default()
        }
    }

    #[test]
    fn anfrage_antwort_mapping() {
        assert_eq!(antwort_for_anfrage(44139), Some(44142)); // LF → NB
        assert_eq!(antwort_for_anfrage(44140), Some(44142)); // MSB → NB
        assert_eq!(antwort_for_anfrage(44137), Some(44138)); // NB → LF
        assert_eq!(antwort_for_anfrage(44162), Some(44163)); // LF → NB [Verteiler]
        // UTILMD AHB Gas 1.2 Kap. 5.12.7, S. 206: the MSB answers both the
        // forwarded 44165 and the NB's own 44166 with 44167 — 44166 is an
        // Anfrage, not an Antwort.
        assert_eq!(antwort_for_anfrage(44165), Some(44167));
        assert_eq!(antwort_for_anfrage(44166), Some(44167));
        assert!(!is_anfrage_response_pid(44166));
        assert!(is_anfrage_request_pid(44139));
        assert!(!is_anfrage_request_pid(44112)); // an Änderung, not an Anfrage
    }

    /// An Anfrage zur Stammdatenänderung is checked before it is answered —
    /// „Nach Prüfung durch den verantwortlichen NB wird das fachliche Ergebnis
    /// der Anfrage in die Antwort … übernommen" (AWH GeLi Gas V1.2 Kap. 4.4.2
    /// Nr. 2) — so its arrival sends nothing and opens the 10-WT window.
    #[test]
    fn an_anfrage_opens_its_antwort_window_and_sends_nothing() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            anfrage_cmd(44139, true),
        )
        .unwrap();
        assert!(out.outbox.is_empty(), "{:?}", out.outbox);
        assert_eq!(out.deadlines.len(), 1);
        assert_eq!(&*out.deadlines[0].label, ANTWORT_WINDOW_LABEL);
        let state = apply_all(GasStammdatenState::New, &out.events);
        let GasStammdatenState::AnfrageOffen(a) = &state else {
            panic!("{state:?}");
        };
        assert_eq!(a.antwort_pid.as_u32(), 44142);
        assert!(!state.is_terminal());
    }

    /// The Antwort restates the Anfrage's dates, Grund and Vorgangsnummer and
    /// carries the Stammdaten the boundary read.
    #[test]
    fn the_antwort_carries_the_stammdaten_it_is_given() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &offen(),
            GasStammdatenCommand::SendAnfrageAntwort {
                antwort_code: "ZG2".to_owned(),
                stammdaten: gemeinderabatt(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1);
        let p: UtilmdPayload = serde_json::from_value(out.outbox[0].payload.clone()).unwrap();
        assert_eq!(p.pid, 44142);
        assert_eq!(p.sender, "9800357000004");
        assert_eq!(p.receiver.as_deref(), Some("9800357000011"));
        assert_eq!(p.antwort_code.as_deref(), Some("ZG2"));
        assert_eq!(p.antwort_codeliste.as_deref(), Some("G_0031"));
        assert_eq!(p.process_date.as_deref(), Some("20260801"));
        assert_eq!(p.vertragsbeginn.as_deref(), Some("20260101"));
        assert_eq!(p.transaktionsgrund.as_deref(), Some("ZF4"));
        assert_eq!(p.referenz_vorgangsnummer.as_deref(), Some("ANF-VG-1"));
        assert_eq!(p.stammdaten, gemeinderabatt());
        let state = apply_all(offen(), &out.events);
        assert!(state.is_terminal(), "{state:?}");
    }

    /// Bedingung `[202]`: only `ZG2` carries data groups.
    #[test]
    fn an_ablehnung_carries_no_stammdaten() {
        let refused = GeliGasStammdatenaenderungWorkflow::handle(
            &offen(),
            GasStammdatenCommand::SendAnfrageAntwort {
                antwort_code: "ZG0".to_owned(),
                stammdaten: gemeinderabatt(),
            },
        );
        assert!(refused.is_err());
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &offen(),
            GasStammdatenCommand::SendAnfrageAntwort {
                antwort_code: "ZG0".to_owned(),
                stammdaten: Stammdaten::default(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["antwort_code"], "ZG0");
    }

    /// The Verantwortlicher missing its 10 WT is a Frist breach the operator
    /// acts on; a late answer is still accepted.
    #[test]
    fn an_unanswered_anfrage_lapses_into_a_frist_breach() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &offen(),
            GasStammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let lapsed = apply_all(offen(), &out.events);
        assert!(matches!(lapsed, GasStammdatenState::FristAbgelaufen { .. }));
        let late = GeliGasStammdatenaenderungWorkflow::handle(
            &lapsed,
            GasStammdatenCommand::SendAnfrageAntwort {
                antwort_code: "ZG0".to_owned(),
                stammdaten: Stammdaten::default(),
            },
        );
        assert!(late.is_ok());
    }

    fn offen_auf(anfrage_pid: u32) -> GasStammdatenState {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            anfrage_cmd(anfrage_pid, true),
        )
        .unwrap();
        apply_all(GasStammdatenState::New, &out.events)
    }

    fn antworte(anfrage_pid: u32, code: &str, stammdaten: Stammdaten) -> UtilmdPayload {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &offen_auf(anfrage_pid),
            GasStammdatenCommand::SendAnfrageAntwort {
                antwort_code: code.to_owned(),
                stammdaten,
            },
        )
        .unwrap_or_else(|e| panic!("{anfrage_pid} {code}: {e}"));
        serde_json::from_value(out.outbox[0].payload.clone()).unwrap()
    }

    /// The code picks the Antwort column: its PID and the Codeliste its
    /// `STS+E01` names (UTILMD AHB Gas 1.2 Kap. 5.12, `SG4 STS 1131`).
    #[test]
    fn the_code_picks_the_antwort_column() {
        for (anfrage, code, pid, liste) in [
            (44139, "ZG2", 44142, "G_0031"),
            (44140, "ZG0", 44142, "G_0069"),
            (44156, "ZG2", 44157, "G_0034"),
            (44156, "ZG4", 44157, "G_0034"),
            (44180, "ZG2", 44181, "G_0035"),
            (44180, "ZJ5", 44182, "G_0036"),
            (44137, "ZG0", 44138, "G_0040"),
            (44150, "ZG2", 44151, "G_0038"),
            (44150, "ZG0", 44152, "G_0039"),
        ] {
            let p = antworte(anfrage, code, Stammdaten::default());
            assert_eq!(p.pid, pid, "{anfrage} {code}");
            assert_eq!(
                p.antwort_codeliste.as_deref(),
                Some(liste),
                "{anfrage} {code}"
            );
        }
    }

    /// A code the column's Codeliste does not publish is refused: `ZG4` is no
    /// `G_0031` code, `ZG0` no `G_0034`/`G_0035` one, `ZJ5` only `G_0036`'s.
    #[test]
    fn a_code_outside_the_columns_codelisten_is_refused() {
        for (anfrage, code) in [
            (44139, "ZG4"),
            (44156, "ZG0"),
            (44180, "ZG0"),
            (44137, "ZJ5"),
            (44150, "E15"),
        ] {
            let r = GeliGasStammdatenaenderungWorkflow::handle(
                &offen_auf(anfrage),
                GasStammdatenCommand::SendAnfrageAntwort {
                    antwort_code: code.to_owned(),
                    stammdaten: Stammdaten::default(),
                },
            );
            assert!(r.is_err(), "{anfrage} {code}");
        }
    }

    /// `ZG4` transmits „die zum angefragten Datum gültigen Stammdaten" (EBD
    /// 4.3, S. 743), so it carries data like `ZG2`.
    #[test]
    fn zg4_carries_stammdaten() {
        let p = antworte(44156, "ZG4", gemeinderabatt());
        assert_eq!(p.stammdaten, gemeinderabatt());
    }

    /// The Ablehnungen 44152 and 44182 list no `SG4 DTM` (UTILMD AHB Gas 1.2
    /// Kap. 5.12.3/5.12.5), and the 44182 names the Messlokationen (`[590]`).
    #[test]
    fn an_ablehnung_pid_without_dates_restates_none() {
        let p = antworte(44150, "ZG0", Stammdaten::default());
        assert_eq!((p.process_date, p.vertragsbeginn), (None, None));
        let melos = Stammdaten {
            lokationen: vec![mako_engine::payload::StammdatenLokation {
                qualifier: "172".to_owned(),
                id: "DE0000000000000000000000000000042".to_owned(),
            }],
            ..Stammdaten::default()
        };
        let p = antworte(44180, "ZJ5", melos.clone());
        assert_eq!(p.pid, 44182);
        assert_eq!(p.process_date, None);
        assert_eq!(p.stammdaten, melos);
        let p = antworte(44180, "ZG2", Stammdaten::default());
        assert_eq!(p.process_date.as_deref(), Some("20260801"));
    }

    /// Every Codeliste an Antwort column names publishes codes, every one of
    /// them through the catalogue, and each Ablehnung PID is an Anfrage
    /// response, so an inbound one is never applied as an Änderung.
    #[test]
    fn every_antwort_column_resolves_through_the_catalogue() {
        for r in ANFRAGE_ANTWORTEN {
            assert!(is_anfrage_request_pid(r.anfrage_pid), "{r:?}");
            assert!(is_anfrage_response_pid(r.antwort_pid), "{r:?}");
            assert!(
                ["ZG0", "ZG2", "ZG4", "ZJ5"]
                    .iter()
                    .any(|c| mako_pruefung::codes::lies(r.codeliste, c).is_some()),
                "{} publishes none of the Anfrage codes",
                r.codeliste
            );
        }
        assert!(mako_pruefung::codes::lies("G_0031", "ZG4").is_none());
    }

    #[test]
    fn anfrage_validation_failure_rejects() {
        let out = GeliGasStammdatenaenderungWorkflow::handle(
            &GasStammdatenState::New,
            anfrage_cmd(44139, false),
        )
        .unwrap();
        assert!(matches!(
            out.events.as_slice(),
            [GasStammdatenEvent::Rejected { .. }]
        ));
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
    }
}
